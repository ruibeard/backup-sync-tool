//! Blocking client for the two sync endpoints: list the customer folder and
//! sign URLs. Laravel keeps no file records; the bucket is the index.

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// One object in the customer folder.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    /// Object ETag without quotes; the file's MD5 for single-part uploads.
    pub etag: String,
    /// ISO 8601 UTC, e.g. `2026-01-02T03:04:05.000Z`.
    pub modified: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Put,
    Delete,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ApiError {
    Auth(String),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(m) | Self::Other(m) => write!(f, "{m}"),
        }
    }
}

impl From<ApiError> for String {
    fn from(value: ApiError) -> Self {
        value.to_string()
    }
}

pub struct SyncApiClient {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl SyncApiClient {
    pub fn new(pair_api_base: &str, device_token: &str) -> Self {
        Self {
            base: pair_api_base.trim_end_matches('/').to_string(),
            token: device_token.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(8))
                .timeout_read(Duration::from_secs(30))
                .timeout_write(Duration::from_secs(30))
                .max_idle_connections_per_host(4)
                .build(),
        }
    }

    /// Every file in the customer folder.
    pub fn list_files(&self) -> Result<Vec<RemoteFile>, ApiError> {
        let resp = self
            .agent
            .get(&format!("{}/api/sync/files", self.base))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(map_ureq)?;
        parse_files(&read_json(resp)?)
    }

    /// Presigned URLs for one method, one per path, in request order.
    pub fn sign(&self, method: Method, paths: &[String]) -> Result<Vec<String>, ApiError> {
        let body = json!({ "method": method.as_str(), "paths": paths });
        let resp = self
            .agent
            .post(&format!("{}/api/sync/sign", self.base))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(map_ureq)?;
        signed_urls(&read_json(resp)?, paths.len())
    }
}

fn parse_files(parsed: &serde_json::Value) -> Result<Vec<RemoteFile>, ApiError> {
    let files = parsed
        .get("files")
        .cloned()
        .ok_or_else(|| ApiError::Other("response without files".into()))?;
    serde_json::from_value(files).map_err(|e| ApiError::Other(format!("files: {e}")))
}

/// Read `urls` from a response; it must hold exactly `expected` strings.
fn signed_urls(parsed: &serde_json::Value, expected: usize) -> Result<Vec<String>, ApiError> {
    let urls = parsed
        .get("urls")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::Other("response without urls".into()))?;
    let urls: Vec<String> = urls
        .iter()
        .filter_map(|u| u.as_str().map(str::to_string))
        .collect();
    if urls.len() != expected {
        return Err(ApiError::Other(format!(
            "expected {expected} signed URLs, got {}",
            urls.len()
        )));
    }
    Ok(urls)
}

fn map_ureq(err: ureq::Error) -> ApiError {
    match err {
        ureq::Error::Status(401 | 403, resp) => {
            let body = resp.into_string().unwrap_or_default();
            ApiError::Auth(format!("HTTP auth failure: {body}"))
        }
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            ApiError::Other(format!("HTTP {code}: {body}"))
        }
        other => ApiError::Other(other.to_string()),
    }
}

fn read_json(resp: ureq::Response) -> Result<serde_json::Value, ApiError> {
    let body = resp
        .into_string()
        .map_err(|e| ApiError::Other(format!("body: {e}")))?;
    serde_json::from_str(&body).map_err(|e| ApiError::Other(format!("json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_list() {
        let body = json!({ "files": [
            { "path": "a/b.txt", "size": 3, "etag": "abc", "modified": "2026-01-02T03:04:05.000Z" }
        ]});
        let files = parse_files(&body).unwrap();
        assert_eq!(files[0].path, "a/b.txt");
        assert_eq!(files[0].size, 3);
        assert!(parse_files(&json!({})).is_err());
    }

    #[test]
    fn signed_urls_keep_order_and_count() {
        let body = json!({ "urls": ["https://s/a?sig", "https://s/b?sig"], "expires_in": 3600 });
        assert_eq!(
            signed_urls(&body, 2).unwrap(),
            vec!["https://s/a?sig".to_string(), "https://s/b?sig".to_string()]
        );
        // A short or missing list must fail loudly, not touch the wrong file.
        assert!(signed_urls(&body, 3).is_err());
        assert!(signed_urls(&json!({ "missing": [] }), 0).is_err());
    }
}
