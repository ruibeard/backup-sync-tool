//! Blocking Laravel sync metadata client.

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteChange {
    pub cursor: u64,
    pub file_id: String,
    pub path: String,
    pub revision: u64,
    pub op: String,
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

    pub fn cursor(&self) -> Result<u64, String> {
        let url = format!("{}/api/sync/cursor", self.base);
        let body = self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| format!("cursor: {e}"))?
            .into_string()
            .map_err(|e| format!("cursor body: {e}"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("cursor json: {e}"))?;
        Ok(parsed
            .get("cursor")
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }

    pub fn changes(&self, since: u64) -> Result<Vec<RemoteChange>, String> {
        let url = format!("{}/api/sync/changes?since={since}", self.base);
        let body = self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| format!("changes: {e}"))?
            .into_string()
            .map_err(|e| format!("changes body: {e}"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("changes json: {e}"))?;
        let Some(items) = parsed.get("changes").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for item in items {
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
            });
        }
        Ok(out)
    }

    pub fn chunks_present(&self, hashes: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
        let url = format!("{}/api/sync/chunks/present", self.base);
        let body = json!({ "hashes": hashes }).to_string();
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("chunks/present: {e}"))?
            .into_string()
            .map_err(|e| format!("chunks/present body: {e}"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&resp).map_err(|e| format!("chunks/present json: {e}"))?;
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
    ) -> Result<(), String> {
        let url = format!("{}/api/sync/commit", self.base);
        let body = json!({
            "path": path,
            "size": size,
            "content_sha256": content_sha256,
            "chunk_hashes": chunk_hashes,
        })
        .to_string();
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("commit: {e}"))?;
        if resp.status() >= 300 {
            return Err(format!("commit HTTP {}", resp.status()));
        }
        Ok(())
    }
}
