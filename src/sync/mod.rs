//! In-process Option H sync engine (chunk metadata + object store bytes).

mod chunker;
mod client;

pub use chunker::chunk_bytes;
pub use client::SyncApiClient;

use crate::config::{self, Config};
use crate::logs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    Idle,
    Syncing,
    Offline,
    AuthError,
    Failed(String),
}

pub struct SyncEngine {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SyncEngine {
    pub fn start(cfg: Config) -> Result<Self, String> {
        if !config::is_paired(&cfg) {
            return Err("Not paired.".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("chunk-sync".into())
            .spawn(move || run_loop(cfg, stop_thread))
            .map_err(|e| format!("Could not start sync thread: {e}"))?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_loop(cfg: Config, stop: Arc<AtomicBool>) {
    let device_token = match crate::secret::decrypt(&cfg.device_token_enc) {
        Ok(token) => token,
        Err(err) => {
            logs::append(&format!("sync: device token decrypt failed: {err}"));
            return;
        }
    };
    let client = SyncApiClient::new(&cfg.pair_api_base, &device_token);
    let mut cursor = 0u64;
    while !stop.load(Ordering::Acquire) {
        match client.cursor() {
            Ok(remote) => cursor = remote.max(cursor),
            Err(err) => {
                logs::append(&format!("sync: cursor failed: {err}"));
                sleep_interruptible(&stop, Duration::from_secs(5));
                continue;
            }
        }
        if let Err(err) = push_local_changes(&cfg, &client) {
            logs::append(&format!("sync: push failed: {err}"));
        }
        match client.changes(cursor) {
            Ok(changes) => {
                for change in changes {
                    cursor = cursor.max(change.cursor);
                    if let Err(err) = apply_remote_change(&cfg, &change) {
                        logs::append(&format!("sync: apply failed: {err}"));
                    }
                }
            }
            Err(err) => logs::append(&format!("sync: changes failed: {err}")),
        }
        sleep_interruptible(&stop, Duration::from_secs(3));
    }
}

fn push_local_changes(cfg: &Config, client: &SyncApiClient) -> Result<(), String> {
    let root = Path::new(cfg.watch_folder.trim());
    if !root.is_dir() {
        return Ok(());
    }
    // Initial skeleton: one-shot scan of top-level files only.
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "bad file name".to_string())?
            .to_string();
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let chunks = chunk_bytes(&bytes);
        let hashes: Vec<String> = chunks.iter().map(|c| c.sha256_hex.clone()).collect();
        let present = client.chunks_present(&hashes)?;
        // Chunk PUT to object store lands in a follow-up commit; metadata commit first.
        let _ = present;
        let content_sha = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        };
        client.commit_file(&rel, bytes.len() as u64, &content_sha, &hashes)?;
    }
    Ok(())
}

fn apply_remote_change(_cfg: &Config, change: &client::RemoteChange) -> Result<(), String> {
    logs::append(&format!(
        "sync: remote {} {} rev={}",
        change.op, change.path, change.revision
    ));
    Ok(())
}

fn sleep_interruptible(stop: &AtomicBool, total: Duration) {
    let mut left = total;
    let step = Duration::from_millis(200);
    while left > Duration::ZERO && !stop.load(Ordering::Acquire) {
        let slice = step.min(left);
        thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}
