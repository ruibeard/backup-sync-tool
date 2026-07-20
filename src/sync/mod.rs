//! In-process Option H sync engine (chunk metadata + object store bytes).

mod chunker;
mod client;
mod state;
mod store;

pub use chunker::chunk_bytes;
pub use client::SyncApiClient;
pub use store::ChunkStoreClient;

use crate::config::{self, Config};
use crate::logs;
use client::{ApiError, RemoteChange};
use sha2::{Digest, Sha256};
use state::{state_path_for_destination, FileTip, SyncState};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
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

fn run_loop(cfg: Config, stop: Arc<AtomicBool>) {
    let device_token = match crate::secret::decrypt(&cfg.device_token_enc) {
        Ok(token) => token,
        Err(err) => {
            logs::append(&format!("sync: device token decrypt failed: {err}"));
            return;
        }
    };
    let access_key = match crate::secret::decrypt(&cfg.chunk_access_key_enc) {
        Ok(v) => v,
        Err(err) => {
            logs::append(&format!("sync: chunk access key decrypt failed: {err}"));
            return;
        }
    };
    let secret_key = match crate::secret::decrypt(&cfg.chunk_secret_key_enc) {
        Ok(v) => v,
        Err(err) => {
            logs::append(&format!("sync: chunk secret key decrypt failed: {err}"));
            return;
        }
    };

    let api = SyncApiClient::new(&cfg.pair_api_base, &device_token);
    let store = ChunkStoreClient::new(
        &cfg.chunk_endpoint,
        &cfg.chunk_region,
        &cfg.chunk_bucket,
        &cfg.chunk_prefix,
        &access_key,
        &secret_key,
        cfg.chunk_path_style,
    );
    let state_path = state_path_for_destination(&cfg.destination_uuid);
    let mut state = SyncState::load(&state_path);

    while !stop.load(Ordering::Acquire) {
        match api.cursor() {
            Ok(remote) => {
                // Pull advances local cursor; remote tip is informational only.
                let _ = remote;
            }
            Err(ApiError::Auth(err)) => {
                logs::append(&format!("sync: auth error on cursor: {err}"));
                return;
            }
            Err(err) => {
                logs::append(&format!("sync: cursor failed: {err}"));
                sleep_interruptible(&stop, Duration::from_secs(5));
                continue;
            }
        }

        if let Err(err) = push_local_changes(&cfg, &api, &store, &mut state) {
            match &err {
                ApiError::Auth(msg) => {
                    logs::append(&format!("sync: auth error on push: {msg}"));
                    let _ = state.save(&state_path);
                    return;
                }
                other => logs::append(&format!("sync: push failed: {other}")),
            }
        }

        match pull_remote_changes(&cfg, &api, &store, &mut state) {
            Ok(()) => {}
            Err(ApiError::Auth(err)) => {
                logs::append(&format!("sync: auth error on pull: {err}"));
                let _ = state.save(&state_path);
                return;
            }
            Err(err) => logs::append(&format!("sync: pull failed: {err}")),
        }

        if let Err(err) = state.save(&state_path) {
            logs::append(&format!("sync: state save failed: {err}"));
        }
        sleep_interruptible(&stop, Duration::from_secs(3));
    }
}

fn push_local_changes(
    cfg: &Config,
    api: &SyncApiClient,
    store: &ChunkStoreClient,
    state: &mut SyncState,
) -> Result<(), ApiError> {
    let root = Path::new(cfg.watch_folder.trim());
    if !root.is_dir() {
        return Ok(());
    }

    let local_files = scan_files(root).map_err(ApiError::Other)?;
    let local_paths: HashSet<String> = local_files.keys().cloned().collect();

    for (rel, abs) in &local_files {
        let bytes = fs::read(abs).map_err(|e| ApiError::Other(e.to_string()))?;
        let content_sha = sha256_hex(&bytes);
        if let Some(tip) = state.files.get(rel) {
            if tip.content_sha256 == content_sha && tip.size == bytes.len() as u64 {
                continue;
            }
        }

        let chunks = chunk_bytes(&bytes);
        let hashes: Vec<String> = chunks.iter().map(|c| c.sha256_hex.clone()).collect();
        if !hashes.is_empty() {
            let (_present, missing) = api.chunks_present(&hashes)?;
            let by_hash: HashMap<&str, &[u8]> = chunks
                .iter()
                .map(|c| (c.sha256_hex.as_str(), c.data.as_slice()))
                .collect();
            for hash in &missing {
                let data = by_hash
                    .get(hash.as_str())
                    .ok_or_else(|| ApiError::Other(format!("missing chunk bytes for {hash}")))?;
                store
                    .put_chunk(hash, data)
                    .map_err(ApiError::Other)?;
            }
        }

        let base_revision = state.files.get(rel).map(|t| t.revision);
        let file_id = state.files.get(rel).map(|t| t.file_id.clone());
        let result = api.commit_file(
            rel,
            bytes.len() as u64,
            &content_sha,
            &hashes,
            file_id.as_deref(),
            base_revision,
            false,
        )?;
        state.cursor = state.cursor.max(result.cursor);
        state.upsert_tip(
            &result.path,
            FileTip {
                file_id: result.file_id,
                revision: result.revision,
                size: bytes.len() as u64,
                content_sha256: content_sha,
            },
        );
        logs::append(&format!("sync: committed {}", result.path));
    }

    let deleted: Vec<(String, FileTip)> = state
        .files
        .iter()
        .filter(|(path, _)| !local_paths.contains(*path))
        .map(|(path, tip)| (path.clone(), tip.clone()))
        .collect();
    for (rel, tip) in deleted {
        let result = api.commit_file(
            &rel,
            0,
            "",
            &[],
            Some(&tip.file_id),
            Some(tip.revision),
            true,
        )?;
        state.cursor = state.cursor.max(result.cursor);
        state.remove_path(&rel);
        logs::append(&format!("sync: deleted {rel}"));
    }

    Ok(())
}

fn pull_remote_changes(
    cfg: &Config,
    api: &SyncApiClient,
    store: &ChunkStoreClient,
    state: &mut SyncState,
) -> Result<(), ApiError> {
    let changes = api.changes(state.cursor)?;
    for change in changes {
        state.cursor = state.cursor.max(change.cursor);
        if let Err(err) = apply_remote_change(cfg, store, state, &change) {
            logs::append(&format!(
                "sync: apply failed for {} ({}): {err}",
                change.path, change.op
            ));
        }
    }
    Ok(())
}

fn apply_remote_change(
    cfg: &Config,
    store: &ChunkStoreClient,
    state: &mut SyncState,
    change: &RemoteChange,
) -> Result<(), String> {
    let root = Path::new(cfg.watch_folder.trim());
    if cfg.watch_folder.trim().is_empty() {
        return Err("watch folder empty".into());
    }

    let deleted = change.op.eq_ignore_ascii_case("delete") || change.payload.deleted;
    if deleted {
        if let Some(old_path) = state.path_for_file_id(&change.file_id) {
            let old = old_path.to_string();
            remove_local_file(root, &old)?;
            state.remove_path(&old);
        }
        remove_local_file(root, &change.path)?;
        state.remove_path(&change.path);
        logs::append(&format!("sync: remote delete {}", change.path));
        return Ok(());
    }

    // Rename: same file_id, new path — remove the old local file first.
    if let Some(old_path) = state
        .path_for_file_id(&change.file_id)
        .map(str::to_string)
    {
        if old_path != change.path {
            remove_local_file(root, &old_path)?;
            state.remove_path(&old_path);
            logs::append(&format!(
                "sync: remote rename {old_path} -> {}",
                change.path
            ));
        }
    }

    let content_sha = change
        .payload
        .content_sha256
        .clone()
        .unwrap_or_default();
    if let Some(tip) = state.files.get(&change.path) {
        if tip.revision >= change.revision
            && tip.content_sha256 == content_sha
            && tip.file_id == change.file_id
        {
            return Ok(());
        }
    }

    // Skip re-download when this device already has identical bytes.
    if change
        .payload
        .updated_by_device_uuid
        .as_deref()
        .is_some_and(|id| id == cfg.device_uuid)
    {
        if let Some(tip) = state.files.get(&change.path) {
            if tip.content_sha256 == content_sha {
                state.upsert_tip(
                    &change.path,
                    FileTip {
                        file_id: change.file_id.clone(),
                        revision: change.revision,
                        size: change.payload.size,
                        content_sha256: content_sha,
                    },
                );
                return Ok(());
            }
        }
    }

    let mut assembled = Vec::with_capacity(change.payload.size as usize);
    for hash in &change.payload.chunk_hashes {
        let chunk = store.get_chunk(hash)?;
        assembled.extend_from_slice(&chunk);
    }
    if change.payload.size > 0 && assembled.len() as u64 != change.payload.size {
        return Err(format!(
            "size mismatch for {}: got {} expected {}",
            change.path,
            assembled.len(),
            change.payload.size
        ));
    }
    if !content_sha.is_empty() {
        let got = sha256_hex(&assembled);
        if !got.eq_ignore_ascii_case(&content_sha) {
            return Err(format!(
                "content hash mismatch for {}: expected {content_sha}, got {got}",
                change.path
            ));
        }
    }

    write_local_file(root, &change.path, &assembled)?;
    state.upsert_tip(
        &change.path,
        FileTip {
            file_id: change.file_id.clone(),
            revision: change.revision,
            size: assembled.len() as u64,
            content_sha256: if content_sha.is_empty() {
                sha256_hex(&assembled)
            } else {
                content_sha
            },
        },
    );
    logs::append(&format!(
        "sync: remote upsert {} rev={}",
        change.path, change.revision
    ));
    Ok(())
}

fn scan_files(root: &Path) -> Result<HashMap<String, PathBuf>, String> {
    let mut out = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "." || name == ".." {
                continue;
            }
            if name.ends_with(".bst-tmp") {
                continue;
            }
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map_err(|_| "path outside watch root".to_string())?;
            let rel = normalize_rel_path(rel)?;
            out.insert(rel, path);
        }
    }
    Ok(out)
}

fn normalize_rel_path(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(s) => {
                let s = s.to_string_lossy();
                if s == ".." || s.contains('\\') {
                    return Err(format!("invalid path component: {s}"));
                }
                parts.push(s.to_string());
            }
            std::path::Component::CurDir => {}
            _ => return Err(format!("invalid path: {}", path.display())),
        }
    }
    if parts.is_empty() {
        return Err("empty relative path".into());
    }
    Ok(parts.join("/"))
}

fn write_local_file(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
    let abs = safe_join(root, rel)?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = abs.with_extension(format!(
        "{}bst-tmp",
        abs.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{e}."))
            .unwrap_or_default()
    ));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("create temp: {e}"))?;
        f.write_all(bytes).map_err(|e| format!("write temp: {e}"))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, &abs).map_err(|e| format!("rename into place: {e}"))?;
    Ok(())
}

fn remove_local_file(root: &Path, rel: &str) -> Result<(), String> {
    if rel.trim().is_empty() {
        return Ok(());
    }
    let abs = safe_join(root, rel)?;
    match fs::remove_file(&abs) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove {}: {err}", abs.display())),
    }
}

fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|p| p == "..") {
        return Err(format!("unsafe relative path: {rel}"));
    }
    let mut abs = root.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        abs.push(part);
    }
    Ok(abs)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_nested_relative_path() {
        let p = Path::new("a").join("b").join("c.txt");
        assert_eq!(normalize_rel_path(&p).unwrap(), "a/b/c.txt");
    }

    #[test]
    fn safe_join_rejects_dotdot() {
        assert!(safe_join(Path::new("/tmp/watch"), "../x").is_err());
    }

    #[test]
    fn scan_files_is_recursive() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bst-scan-{nanos}"));
        fs::create_dir_all(root.join("sub/nested")).unwrap();
        fs::write(root.join("top.txt"), b"a").unwrap();
        fs::write(root.join("sub/nested/deep.txt"), b"b").unwrap();
        let files = scan_files(&root).unwrap();
        assert!(files.contains_key("top.txt"));
        assert!(files.contains_key("sub/nested/deep.txt"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_and_remove_nested_file() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bst-write-{nanos}"));
        fs::create_dir_all(&root).unwrap();
        write_local_file(&root, "x/y/z.txt", b"hello").unwrap();
        assert_eq!(fs::read(root.join("x/y/z.txt")).unwrap(), b"hello");
        remove_local_file(&root, "x/y/z.txt").unwrap();
        assert!(!root.join("x/y/z.txt").exists());
        let _ = fs::remove_dir_all(root);
    }
}
