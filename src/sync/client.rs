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
    pub chunk_hashes: Vec<String>,
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
pub struct CommitResult {
    pub file_id: String,
    pub path: String,
    pub revision: u64,
    pub cursor: u64,
    pub deleted: bool,
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
                .build(),
        }
    }

    pub fn cursor(&self) -> Result<u64, ApiError> {
        let url = format!("{}/api/sync/cursor", self.base);
        let body = self.get_json(&url)?;
        Ok(body
            .get("cursor")
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }

    pub fn changes(&self, since: u64) -> Result<Vec<RemoteChange>, ApiError> {
        let url = format!("{}/api/sync/changes?since={since}", self.base);
        let parsed = self.get_json(&url)?;
        let Some(items) = parsed.get("changes").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
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
        Ok(out)
    }

    pub fn chunks_present(&self, hashes: &[String]) -> Result<(Vec<String>, Vec<String>), ApiError> {
        let url = format!("{}/api/sync/chunks/present", self.base);
        let body = json!({ "hashes": hashes }).to_string();
        let parsed = self.post_json(&url, &body)?;
        let present = parsed
            .get("present")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let missing = parsed
            .get("missing")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok((present, missing))
    }

    pub fn commit_file(
        &self,
        path: &str,
        size: u64,
        content_sha256: &str,
        chunk_hashes: &[String],
        file_id: Option<&str>,
        base_revision: Option<u64>,
        deleted: bool,
    ) -> Result<CommitResult, ApiError> {
        let url = format!("{}/api/sync/commit", self.base);
        let mut body = json!({
            "path": path,
            "size": size,
            "content_sha256": if content_sha256.is_empty() {
                serde_json::Value::Null
            } else {
                json!(content_sha256)
            },
            "chunk_hashes": chunk_hashes,
            "deleted": deleted,
        });
        if let Some(id) = file_id {
            body["file_id"] = json!(id);
        }
        if let Some(rev) = base_revision {
            body["base_revision"] = json!(rev);
        }
        let parsed = self.post_json(&url, &body.to_string())?;
        Ok(CommitResult {
            file_id: parsed
                .get("file_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            path: parsed
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(path)
                .to_string(),
            revision: parsed
                .get("revision")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cursor: parsed.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0),
            deleted: parsed
                .get("deleted")
                .and_then(|v| v.as_bool())
                .unwrap_or(deleted),
        })
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
            "chunk_hashes": ["bb"],
            "deleted": false,
            "updated_by_device_uuid": "dev-1"
        });
        let payload = parse_payload(value);
        assert_eq!(payload.size, 12);
        assert_eq!(payload.content_sha256.as_deref(), Some("aa"));
        assert_eq!(payload.chunk_hashes, vec!["bb".to_string()]);
        assert_eq!(payload.updated_by_device_uuid.as_deref(), Some("dev-1"));
    }
}
