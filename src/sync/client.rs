//! Blocking Laravel sync metadata client.

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChangePayload {
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub content_sha256: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub updated_by_device_uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemoteChange {
    pub cursor: u64,
    pub file_id: String,
    pub path: String,
    pub revision: u64,
    pub op: String,
    pub payload: ChangePayload,
}

#[derive(Debug, Clone)]
pub struct ChangesPage {
    /// Server tip cursor for the destination.
    pub cursor: u64,
    pub changes: Vec<RemoteChange>,
}

/// One file in a `files/upload` request.
#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub path: String,
    pub size: u64,
    pub content_sha256: String,
}

/// One file in a `commit/batch` request.
#[derive(Debug, Clone)]
pub struct CommitItem {
    pub path: String,
    pub size: u64,
    pub content_sha256: String,
    /// Object version from the PUT response (`x-amz-version-id`), if any.
    pub version_id: Option<String>,
    pub file_id: Option<String>,
    pub base_revision: Option<u64>,
    pub deleted: bool,
}

impl CommitItem {
    fn to_json(&self) -> serde_json::Value {
        let mut body = json!({
            "path": self.path,
            "size": self.size,
            "content_sha256": if self.content_sha256.is_empty() {
                serde_json::Value::Null
            } else {
                json!(self.content_sha256)
            },
            "version_id": self.version_id,
            "deleted": self.deleted,
        });
        if let Some(id) = &self.file_id {
            body["file_id"] = json!(id);
        }
        if let Some(rev) = self.base_revision {
            body["base_revision"] = json!(rev);
        }
        body
    }
}

#[derive(Debug, Clone)]
pub struct CommitResult {
    pub file_id: String,
    pub path: String,
    pub revision: u64,
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

    pub fn cursor(&self) -> Result<u64, ApiError> {
        let url = format!("{}/api/sync/cursor", self.base);
        let body = self.get_json(&url)?;
        Ok(body.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0))
    }

    pub fn changes(&self, since: u64) -> Result<ChangesPage, ApiError> {
        let url = format!("{}/api/sync/changes?since={since}", self.base);
        let parsed = self.get_json(&url)?;
        let cursor = parsed.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0);
        let Some(items) = parsed.get("changes").and_then(|v| v.as_array()) else {
            return Ok(ChangesPage {
                cursor,
                changes: Vec::new(),
            });
        };
        let mut out = Vec::new();
        for item in items {
            let payload = item
                .get("payload")
                .cloned()
                .map(parse_payload)
                .unwrap_or_default();
            out.push(RemoteChange {
                cursor: item.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0),
                file_id: item
                    .get("file_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                path: item
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                revision: item.get("revision").and_then(|v| v.as_u64()).unwrap_or(0),
                op: item
                    .get("op")
                    .and_then(|v| v.as_str())
                    .unwrap_or("upsert")
                    .to_string(),
                payload,
            });
        }
        Ok(ChangesPage {
            cursor,
            changes: out,
        })
    }

    /// Signed PUT URLs, one per file, in request order.
    pub fn upload_urls(&self, files: &[UploadRequest]) -> Result<Vec<String>, ApiError> {
        let url = format!("{}/api/sync/files/upload", self.base);
        let body = json!({
            "files": files
                .iter()
                .map(|f| json!({
                    "path": f.path,
                    "size": f.size,
                    "content_sha256": f.content_sha256,
                }))
                .collect::<Vec<_>>()
        });
        let parsed = self.post_json(&url, &body.to_string())?;
        signed_urls(&parsed, files.len())
    }

    /// Signed GET URLs, one per `(path, version_id)`, in request order.
    pub fn download_urls(
        &self,
        files: &[(String, Option<String>)],
    ) -> Result<Vec<String>, ApiError> {
        let url = format!("{}/api/sync/files/download", self.base);
        let body = json!({
            "files": files
                .iter()
                .map(|(path, version_id)| json!({ "path": path, "version_id": version_id }))
                .collect::<Vec<_>>()
        });
        let parsed = self.post_json(&url, &body.to_string())?;
        signed_urls(&parsed, files.len())
    }

    /// Commit files in order. Each entry is that item's result or its error.
    pub fn commit_batch(
        &self,
        items: &[CommitItem],
    ) -> Result<Vec<Result<CommitResult, String>>, ApiError> {
        let url = format!("{}/api/sync/commit/batch", self.base);
        let body = json!({ "items": items.iter().map(CommitItem::to_json).collect::<Vec<_>>() });
        let parsed = self.post_json(&url, &body.to_string())?;
        let results = parsed
            .get("results")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ApiError::Other("commit/batch: missing results".into()))?;
        if results.len() != items.len() {
            return Err(ApiError::Other(format!(
                "commit/batch: {} results for {} items",
                results.len(),
                items.len()
            )));
        }
        let parsed: Vec<_> = results
            .iter()
            .zip(items)
            .map(|(r, item)| parse_commit_result(r, &item.path))
            .collect();
        // Tips are stored by position; a reordered response would swap files.
        for (result, item) in parsed.iter().zip(items) {
            if let Ok(result) = result {
                if result.path != item.path {
                    return Err(ApiError::Other(format!(
                        "commit/batch: result for {} came back as {}",
                        item.path, result.path
                    )));
                }
            }
        }
        Ok(parsed)
    }

    fn get_json(&self, url: &str) -> Result<serde_json::Value, ApiError> {
        let resp = self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(map_ureq)?;
        read_json(resp)
    }

    fn post_json(&self, url: &str, body: &str) -> Result<serde_json::Value, ApiError> {
        let resp = self
            .agent
            .post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(body)
            .map_err(map_ureq)?;
        read_json(resp)
    }
}

fn parse_commit_result(value: &serde_json::Value, path: &str) -> Result<CommitResult, String> {
    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        return Err(error.to_string());
    }
    let file_id = value
        .get("file_id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "commit result without file_id".to_string())?;
    Ok(CommitResult {
        file_id: file_id.to_string(),
        path: value
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(path)
            .to_string(),
        revision: value.get("revision").and_then(|v| v.as_u64()).unwrap_or(0),
    })
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

fn parse_payload(value: serde_json::Value) -> ChangePayload {
    serde_json::from_value(value).unwrap_or_default()
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
    let status = resp.status();
    let body = resp
        .into_string()
        .map_err(|e| ApiError::Other(format!("body: {e}")))?;
    if status == 401 || status == 403 {
        return Err(ApiError::Auth(format!("HTTP {status}: {body}")));
    }
    if status >= 300 {
        return Err(ApiError::Other(format!("HTTP {status}: {body}")));
    }
    serde_json::from_str(&body).map_err(|e| ApiError::Other(format!("json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_change_payload_fields() {
        let value = serde_json::json!({
            "size": 12,
            "content_sha256": "aa",
            "version_id": "v1",
            "deleted": false,
            "updated_by_device_uuid": "dev-1"
        });
        let payload = parse_payload(value);
        assert_eq!(payload.size, 12);
        assert_eq!(payload.content_sha256.as_deref(), Some("aa"));
        assert_eq!(payload.version_id.as_deref(), Some("v1"));
        assert_eq!(payload.updated_by_device_uuid.as_deref(), Some("dev-1"));
    }

    #[test]
    fn signed_urls_keep_order_and_count() {
        let body = serde_json::json!({ "urls": ["https://s/a?sig", "https://s/b?sig"], "expires_in": 3600 });
        assert_eq!(
            signed_urls(&body, 2).unwrap(),
            vec!["https://s/a?sig".to_string(), "https://s/b?sig".to_string()]
        );
        // A short or missing list must fail loudly, not upload the wrong file.
        assert!(signed_urls(&body, 3).is_err());
        assert!(signed_urls(&serde_json::json!({ "missing": [] }), 0).is_err());
    }

    #[test]
    fn commit_item_json_carries_version_id() {
        let item = CommitItem {
            path: "a/b.txt".into(),
            size: 3,
            content_sha256: "aa".into(),
            version_id: Some("v9".into()),
            file_id: None,
            base_revision: None,
            deleted: false,
        };
        let body = item.to_json();
        assert_eq!(body["version_id"], "v9");
        assert!(body.get("chunk_hashes").is_none());
        let none = CommitItem {
            version_id: None,
            ..item
        };
        assert!(none.to_json()["version_id"].is_null());
    }

    #[test]
    fn commit_result_maps_item_errors() {
        let ok =
            serde_json::json!({ "file_id": "f1", "path": "a.txt", "revision": 2, "cursor": 9 });
        let err = serde_json::json!({ "error": "Invalid path." });
        let parsed = parse_commit_result(&ok, "a.txt").unwrap();
        assert_eq!((parsed.file_id.as_str(), parsed.revision), ("f1", 2));
        assert_eq!(parse_commit_result(&err, "x").unwrap_err(), "Invalid path.");
    }
}
