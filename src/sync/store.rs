//! Chunk PUT/GET against signed URLs from the control plane.
//!
//! The device holds no store keys. Laravel signs short-lived URLs scoped to
//! this destination's chunk prefix; bytes go straight to the object store.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Duration;
use ureq::Agent;

#[derive(Debug, Clone)]
pub struct ChunkStore {
    agent: Agent,
}

impl ChunkStore {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(8))
                .timeout_read(Duration::from_secs(60))
                .timeout_write(Duration::from_secs(60))
                // Keep one warm connection per transfer worker.
                .max_idle_connections_per_host(16)
                .build(),
        }
    }

    pub fn put(&self, url: &str, data: &[u8]) -> Result<(), String> {
        self.agent
            .put(url)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(data)
            .map_err(|e| map_ureq_err("chunk PUT", e))?;
        Ok(())
    }

    /// GET one chunk and check its bytes against `sha256_hex`.
    pub fn get(&self, url: &str, sha256_hex: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| map_ureq_err("chunk GET", e))?;
        let mut bytes = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("chunk GET body: {e}"))?;
        let got = hex::encode(Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(sha256_hex.trim()) {
            return Err(format!(
                "chunk hash mismatch: expected {sha256_hex}, got {got}"
            ));
        }
        Ok(bytes)
    }
}

fn map_ureq_err(op: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(404, _) => format!("{op}: chunk missing in store"),
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            format!("{op} HTTP {code}: {snippet}")
        }
        // Signed URLs carry a signature; never put the URL in a log line.
        ureq::Error::Transport(t) => match t.message() {
            Some(msg) => format!("{op}: {}: {msg}", t.kind()),
            None => format!("{op}: {}", t.kind()),
        },
    }
}
