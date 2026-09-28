//! In-process sync engine (chunk metadata + object store bytes).
//!
//! Each loop pushes local changes in batches (parallel chunk PUT, one
//! `chunks/present` and one `commit/batch` per batch), then pulls remote
//! changes when the server cursor moved (parallel chunk GET into a local
//! cache, then apply in cursor order). Only the pull advances the local
//! cursor, so changes from other devices are never skipped.
//!
//! The device holds no store keys: every chunk PUT/GET uses a short-lived
//! URL that Laravel signs for this destination.

mod chunker;
mod client;
mod pool;
mod state;
mod store;
mod watch;

use crate::app::AppCommand;
use crate::config::{self, Config};
use crate::logs;
use chunker::{chunk_file, ChunkRef};
use client::{ApiError, CommitItem, RemoteChange, SyncApiClient};
use sha2::{Digest, Sha256};
use state::{state_path_for_destination, FileTip, SyncState};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};
use store::ChunkStore;
use watch::FolderWatcher;

/// How often the engine asks the server for remote changes when idle.
const REMOTE_POLL: Duration = Duration::from_secs(4);
const ERROR_BACKOFF: Duration = Duration::from_secs(5);
/// Let writes settle after a file-system event before scanning.
const FS_SETTLE: Duration = Duration::from_millis(500);
const TRANSFER_WORKERS: usize = 8;
const HASH_WORKERS: usize = 4;
/// Files per `commit/batch` (server cap is 200).
const BATCH_FILES: usize = 200;
/// Soft cap on bytes hashed and uploaded per push batch.
const BATCH_BYTES: u64 = 256 * 1024 * 1024;
/// `chunks/present` and `chunks/download` accept up to 2000 hashes.
const PRESENT_BATCH: usize = 2000;
/// Download URLs are signed per group, just before use, so they stay fresh on slow links.
const DOWNLOAD_GROUP: usize = 500;
/// Extra attempts for one chunk PUT/GET before it counts as failed.
const CHUNK_RETRIES: u32 = 2;
/// A remote change that fails this many times is skipped so later changes still apply.
const MAX_APPLY_ATTEMPTS: u32 = 3;
/// Activity lists each file up to this count, then one summary line.
const MAX_ACTIVITY_LINES: usize = 10;

pub struct SyncEngine {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SyncEngine {
    pub fn start(cfg: Config, status: Sender<AppCommand>) -> Result<Self, String> {
        if !config::is_paired(&cfg) {
            return Err("Not paired.".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("chunk-sync".into())
            .spawn(move || run_loop(cfg, stop_thread, Reporter::new(status)))
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

/// Sends engine status to the shared app controller (both UIs render it).
struct Reporter {
    tx: Sender<AppCommand>,
    last: Option<(bool, &'static str, u64, u64, u64)>,
}

impl Reporter {
    fn new(tx: Sender<AppCommand>) -> Self {
        Self { tx, last: None }
    }

    fn status(
        &mut self,
        connected: bool,
        folder_state: &'static str,
        local: usize,
        need_files: u64,
        need_bytes: u64,
    ) {
        let next = (
            connected,
            folder_state,
            local as u64,
            need_files,
            need_bytes,
        );
        if self.last == Some(next) {
            return;
        }
        self.last = Some(next);
        let _ = self.tx.send(AppCommand::EngineStatus {
            connected,
            folder_state: folder_state.into(),
            local_files: local as u64,
            need_files,
            need_bytes,
        });
    }

    fn activity(&self, verb: &str, paths: &[String]) {
        if paths.len() > MAX_ACTIVITY_LINES {
            let _ = self.tx.send(AppCommand::Activity(format!(
                "{verb} {} files",
                paths.len()
            )));
            return;
        }
        for path in paths {
            let _ = self.tx.send(AppCommand::Activity(format!("{verb} {path}")));
        }
    }

    fn failed(&mut self, reason: &str) {
        self.last = None;
        let _ = self.tx.send(AppCommand::EngineFailed(reason.into()));
    }
}

struct Ctx<'a> {
    device_uuid: &'a str,
    root: PathBuf,
    api: SyncApiClient,
    store: ChunkStore,
    cache_dir: PathBuf,
    stop: &'a AtomicBool,
}

impl Ctx<'_> {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

#[derive(Default)]
struct Outcome {
    /// Local state changed and must be saved.
    changed: bool,
    /// Some work failed; try again on the next loop.
    retry: bool,
}

impl Outcome {
    fn merge(&mut self, other: Outcome) {
        self.changed |= other.changed;
        self.retry |= other.retry;
    }
}

/// Tracks repeated failures of the first unapplied remote change.
#[derive(Default)]
struct ApplyRetry {
    cursor: u64,
    attempts: u32,
}

fn run_loop(cfg: Config, stop: Arc<AtomicBool>, mut reporter: Reporter) {
    let device_token = match crate::secret::decrypt(&cfg.device_token_enc) {
        Ok(v) => v,
        Err(err) => {
            logs::append(&format!("sync: device token decrypt failed: {err}"));
            reporter.failed("Stored credentials could not be read. Pair this computer again.");
            return;
        }
    };

    let state_path = state_path_for_destination(&cfg.destination_uuid);
    let ctx = Ctx {
        device_uuid: &cfg.device_uuid,
        root: PathBuf::from(cfg.watch_folder.trim()),
        api: SyncApiClient::new(&cfg.pair_api_base, &device_token),
        store: ChunkStore::new(),
        cache_dir: state_path.with_extension("cache"),
        stop: &stop,
    };
    let _ = fs::remove_dir_all(&ctx.cache_dir);

    let mut state = SyncState::load(&state_path);
    let mut apply_retry = ApplyRetry::default();
    let dirty = Arc::new(AtomicBool::new(true));
    let _watcher = FolderWatcher::start(&ctx.root, Arc::clone(&dirty));

    while !ctx.stopping() {
        if dirty.load(Ordering::Acquire) {
            sleep_interruptible(&stop, None, FS_SETTLE);
        }

        let remote_cursor = match ctx.api.cursor() {
            Ok(cursor) => cursor,
            Err(ApiError::Auth(err)) => {
                logs::append(&format!("sync: auth error on cursor: {err}"));
                reporter.failed("This computer was disconnected. Pair it again.");
                return;
            }
            Err(err) => {
                logs::append(&format!("sync: cursor failed: {err}"));
                reporter.status(false, "offline", state.files.len(), 0, 0);
                sleep_interruptible(&stop, Some(&dirty), ERROR_BACKOFF);
                continue;
            }
        };

        let mut outcome = Outcome::default();
        if dirty.swap(false, Ordering::AcqRel) {
            match push_local_changes(&ctx, &mut state, &mut reporter) {
                Ok(pushed) => outcome.merge(pushed),
                Err(ApiError::Auth(msg)) => {
                    logs::append(&format!("sync: auth error on push: {msg}"));
                    let _ = state.save(&state_path);
                    reporter.failed("This computer was disconnected. Pair it again.");
                    return;
                }
                Err(err) => {
                    logs::append(&format!("sync: push failed: {err}"));
                    outcome.changed = true;
                    outcome.retry = true;
                }
            }
        }

        if remote_cursor > state.cursor {
            match pull_remote_changes(&ctx, &mut state, &mut reporter, &mut apply_retry) {
                Ok(pulled) => outcome.merge(pulled),
                Err(ApiError::Auth(err)) => {
                    logs::append(&format!("sync: auth error on pull: {err}"));
                    let _ = state.save(&state_path);
                    reporter.failed("This computer was disconnected. Pair it again.");
                    return;
                }
                Err(err) => {
                    logs::append(&format!("sync: pull failed: {err}"));
                    outcome.changed = true;
                }
            }
        }

        if outcome.changed {
            if let Err(err) = state.save(&state_path) {
                logs::append(&format!("sync: state save failed: {err}"));
            }
        }
        reporter.status(true, "idle", state.files.len(), 0, 0);
        // Wake at once on FS events; otherwise poll the server cursor.
        sleep_interruptible(&stop, Some(&dirty), REMOTE_POLL);
        if outcome.retry {
            dirty.store(true, Ordering::Release);
        }
    }
    let _ = state.save(&state_path);
    let _ = fs::remove_dir_all(&ctx.cache_dir);
}

struct Candidate {
    rel: String,
    abs: PathBuf,
    size: u64,
    mtime_ns: u64,
}

fn push_local_changes(
    ctx: &Ctx,
    state: &mut SyncState,
    reporter: &mut Reporter,
) -> Result<Outcome, ApiError> {
    let mut out = Outcome::default();
    if !ctx.root.is_dir() {
        return Ok(out);
    }

    let local_files = scan_files(&ctx.root).map_err(ApiError::Other)?;
    let mut candidates = Vec::new();
    for (rel, abs) in &local_files {
        let Ok((size, mtime_ns)) = file_fingerprint(abs) else {
            out.retry = true;
            continue;
        };
        if let Some(tip) = state.files.get(rel) {
            if tip.size == size
                && tip.mtime_ns == mtime_ns
                && mtime_ns > 0
                && !tip.content_sha256.is_empty()
            {
                continue;
            }
        }
        candidates.push(Candidate {
            rel: rel.clone(),
            abs: abs.clone(),
            size,
            mtime_ns,
        });
    }
    let deleted: Vec<(String, FileTip)> = state
        .files
        .iter()
        .filter(|(path, _)| !local_files.contains_key(*path))
        .map(|(path, tip)| (path.clone(), tip.clone()))
        .collect();
    if candidates.is_empty() && deleted.is_empty() {
        return Ok(out);
    }

    let mut need_files = (candidates.len() + deleted.len()) as u64;
    let mut need_bytes: u64 = candidates.iter().map(|c| c.size).sum();
    reporter.status(true, "syncing", state.files.len(), need_files, need_bytes);

    for batch in batches(&candidates) {
        if ctx.stopping() {
            return Ok(out);
        }
        out.merge(push_batch(ctx, state, batch, reporter)?);
        need_files = need_files.saturating_sub(batch.len() as u64);
        need_bytes = need_bytes.saturating_sub(batch.iter().map(|c| c.size).sum());
        reporter.status(true, "syncing", state.files.len(), need_files, need_bytes);
    }

    for group in deleted.chunks(BATCH_FILES) {
        if ctx.stopping() {
            return Ok(out);
        }
        let items: Vec<CommitItem> = group
            .iter()
            .map(|(rel, tip)| CommitItem {
                path: rel.clone(),
                size: 0,
                content_sha256: String::new(),
                chunk_hashes: Vec::new(),
                file_id: Some(tip.file_id.clone()),
                base_revision: Some(tip.revision),
                deleted: true,
            })
            .collect();
        let mut done = Vec::new();
        for ((rel, _), result) in group.iter().zip(ctx.api.commit_batch(&items)?) {
            match result {
                Ok(_) => {
                    state.remove_path(rel);
                    done.push(rel.clone());
                }
                Err(err) => {
                    logs::append(&format!("sync: delete {rel} rejected: {err}"));
                    out.retry = true;
                }
            }
        }
        out.changed |= !done.is_empty();
        logs::append(&format!("sync: deleted {} file(s)", done.len()));
        reporter.activity("Deleted", &done);
        need_files = need_files.saturating_sub(group.len() as u64);
        reporter.status(true, "syncing", state.files.len(), need_files, need_bytes);
    }
    Ok(out)
}

/// Split candidates into batches bounded by file count and bytes.
fn batches(candidates: &[Candidate]) -> Vec<&[Candidate]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bytes = 0u64;
    for (i, c) in candidates.iter().enumerate() {
        let full = i - start >= BATCH_FILES || (i > start && bytes + c.size > BATCH_BYTES);
        if full {
            out.push(&candidates[start..i]);
            start = i;
            bytes = 0;
        }
        bytes += c.size;
    }
    if start < candidates.len() {
        out.push(&candidates[start..]);
    }
    out
}

fn push_batch(
    ctx: &Ctx,
    state: &mut SyncState,
    batch: &[Candidate],
    reporter: &Reporter,
) -> Result<Outcome, ApiError> {
    let mut out = Outcome::default();

    // 1. Hash and chunk in parallel. Chunk bytes are not kept in memory.
    let prepared = pool::parallel_map(batch, HASH_WORKERS, |c| {
        if ctx.stopping() {
            return Err("stopped".to_string());
        }
        chunk_file(&c.abs)
    });
    let mut ready = Vec::new();
    for (c, result) in batch.iter().zip(prepared) {
        let chunks = match result {
            Ok(chunks) => chunks,
            Err(err) => {
                logs::append(&format!("sync: skip {}: {err}", c.rel));
                out.retry = true;
                continue;
            }
        };
        if let Some(tip) = state.files.get(&c.rel) {
            if tip.content_sha256 == chunks.content_sha256 && tip.size == chunks.size {
                // Bytes unchanged; refresh fingerprint so future scans stay cheap.
                let mut tip = tip.clone();
                tip.mtime_ns = c.mtime_ns;
                tip.size = c.size;
                state.upsert_tip(&c.rel, tip);
                out.changed = true;
                continue;
            }
        }
        ready.push((c, chunks));
    }
    if ready.is_empty() {
        return Ok(out);
    }

    // 2. Ask the server which chunks it lacks (deduped across the batch).
    //    Each missing chunk comes back with a signed PUT URL.
    let mut sources: HashMap<&str, (&Path, &ChunkRef)> = HashMap::new();
    for (c, chunks) in &ready {
        for chunk in &chunks.chunks {
            sources
                .entry(chunk.sha256_hex.as_str())
                .or_insert((c.abs.as_path(), chunk));
        }
    }
    let hashes: Vec<String> = sources.keys().map(|h| h.to_string()).collect();
    let mut missing = HashMap::new();
    for group in hashes.chunks(PRESENT_BATCH) {
        missing.extend(ctx.api.missing_chunks(group)?);
    }

    // 3. Upload missing chunks in parallel, reading each slice from disk.
    let jobs: Vec<(&Path, &ChunkRef, &str)> = missing
        .iter()
        .filter_map(|(hash, url)| {
            let (abs, chunk) = sources.get(hash.as_str())?;
            Some((*abs, *chunk, url.as_str()))
        })
        .collect();
    let uploads = pool::parallel_map(&jobs, TRANSFER_WORKERS, |(abs, chunk, url)| {
        if ctx.stopping() {
            return Err("stopped".to_string());
        }
        with_retries(ctx, || ctx.store.put(url, &read_chunk(abs, chunk)?))
    });
    let mut failed: HashSet<&str> = HashSet::new();
    for ((abs, chunk, _), result) in jobs.iter().zip(uploads) {
        if let Err(err) = result {
            logs::append(&format!(
                "sync: upload from {} failed: {err}",
                abs.display()
            ));
            failed.insert(chunk.sha256_hex.as_str());
        }
    }

    // 4. Commit every file whose chunks are all in the store.
    let committable: Vec<_> = ready
        .iter()
        .filter(|(_, chunks)| {
            chunks
                .chunks
                .iter()
                .all(|c| !failed.contains(c.sha256_hex.as_str()))
        })
        .collect();
    if committable.len() < ready.len() {
        out.retry = true;
    }
    if committable.is_empty() {
        return Ok(out);
    }
    let items: Vec<CommitItem> = committable
        .iter()
        .map(|(c, chunks)| {
            let tip = state.files.get(&c.rel);
            CommitItem {
                path: c.rel.clone(),
                size: chunks.size,
                content_sha256: chunks.content_sha256.clone(),
                chunk_hashes: chunks.hashes(),
                file_id: tip.map(|t| t.file_id.clone()),
                base_revision: tip.map(|t| t.revision),
                deleted: false,
            }
        })
        .collect();
    let mut done = Vec::new();
    for ((c, chunks), result) in committable.iter().zip(ctx.api.commit_batch(&items)?) {
        match result {
            Ok(commit) => {
                state.upsert_tip(
                    &commit.path,
                    FileTip {
                        file_id: commit.file_id,
                        revision: commit.revision,
                        size: chunks.size,
                        content_sha256: chunks.content_sha256.clone(),
                        mtime_ns: c.mtime_ns,
                    },
                );
                done.push(commit.path);
            }
            Err(err) => {
                logs::append(&format!("sync: commit {} rejected: {err}", c.rel));
                out.retry = true;
            }
        }
    }
    out.changed |= !done.is_empty();
    logs::append(&format!(
        "sync: committed {} file(s), uploaded {} chunk(s)",
        done.len(),
        jobs.len() - failed.len()
    ));
    reporter.activity("Uploaded", &done);
    Ok(out)
}

/// Retry a chunk transfer in place: stores return 500/503 and drop
/// connections now and then, and one bad chunk should not fail a batch.
fn with_retries<T>(ctx: &Ctx, mut op: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut attempt = 0u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(_) if attempt < CHUNK_RETRIES && !ctx.stopping() => {
                attempt += 1;
                thread::sleep(Duration::from_millis(250 << attempt));
            }
            Err(err) => return Err(err),
        }
    }
}

fn read_chunk(abs: &Path, chunk: &ChunkRef) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(abs).map_err(|e| format!("open {}: {e}", abs.display()))?;
    file.seek(SeekFrom::Start(chunk.offset))
        .map_err(|e| format!("seek {}: {e}", abs.display()))?;
    let mut data = vec![0u8; chunk.len];
    file.read_exact(&mut data)
        .map_err(|e| format!("read {}: {e}", abs.display()))?;
    if sha256_hex(&data) != chunk.sha256_hex {
        return Err(format!("{} changed while syncing", abs.display()));
    }
    Ok(data)
}

fn pull_remote_changes(
    ctx: &Ctx,
    state: &mut SyncState,
    reporter: &mut Reporter,
    apply_retry: &mut ApplyRetry,
) -> Result<Outcome, ApiError> {
    let mut out = Outcome::default();
    while !ctx.stopping() {
        let page = ctx.api.changes(state.cursor)?;
        if page.changes.is_empty() {
            break;
        }
        reporter.status(
            true,
            "syncing",
            state.files.len(),
            page.cursor.saturating_sub(state.cursor),
            page.changes.iter().map(|c| c.payload.size).sum(),
        );
        out.changed = true;
        let complete = apply_remote_page(ctx, state, &page.changes, reporter, apply_retry)?;
        if !complete || state.cursor >= page.cursor {
            break;
        }
    }
    Ok(out)
}

/// Apply one page of remote changes in cursor order. Returns false when a
/// change failed and the page stopped early (it is retried next loop).
fn apply_remote_page(
    ctx: &Ctx,
    state: &mut SyncState,
    changes: &[RemoteChange],
    reporter: &Reporter,
    apply_retry: &mut ApplyRetry,
) -> Result<bool, ApiError> {
    // Only the last change per file in a page matters.
    let mut last_for_file: HashMap<&str, u64> = HashMap::new();
    for change in changes {
        last_for_file.insert(change.file_id.as_str(), change.cursor);
    }
    let superseded = |c: &RemoteChange| last_for_file.get(c.file_id.as_str()) != Some(&c.cursor);

    // Plan the chunks this page needs; reuse matching chunks from old local copies.
    let mut needed: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut reuse: HashMap<String, (PathBuf, ChunkRef)> = HashMap::new();
    for change in changes {
        if superseded(change) || !needs_bytes(ctx, state, change) {
            continue;
        }
        for hash in &change.payload.chunk_hashes {
            if seen.insert(hash.as_str()) {
                needed.push(hash.as_str());
            }
        }
        if change.payload.chunk_hashes.len() > 1 {
            if let Ok(abs) = safe_join(&ctx.root, &change.path) {
                if let Ok(local) = chunk_file(&abs) {
                    for chunk in local.chunks {
                        reuse
                            .entry(chunk.sha256_hex.clone())
                            .or_insert((abs.clone(), chunk));
                    }
                }
            }
        }
    }

    if !needed.is_empty() {
        if let Err(err) = fs::create_dir_all(&ctx.cache_dir) {
            logs::append(&format!("sync: cache dir: {err}"));
            return Ok(false);
        }
    }
    for group in needed.chunks(DOWNLOAD_GROUP) {
        if ctx.stopping() {
            break;
        }
        let hashes: Vec<String> = group.iter().map(|h| h.to_string()).collect();
        let urls = ctx.api.download_urls(&hashes)?;
        let fills = pool::parallel_map(group, TRANSFER_WORKERS, |hash| {
            if ctx.stopping() {
                return Err("stopped".to_string());
            }
            fill_cache(ctx, hash, reuse.get(*hash), urls.get(*hash))
        });
        for (hash, result) in group.iter().zip(fills) {
            if let Err(err) = result {
                logs::append(&format!("sync: chunk {hash} fetch failed: {err}"));
            }
        }
    }

    let mut downloaded = Vec::new();
    let mut removed = Vec::new();
    let mut complete = true;
    for change in changes {
        if ctx.stopping() {
            complete = false;
            break;
        }
        let result = if superseded(change) {
            Ok(None)
        } else {
            apply_remote_change(ctx, state, change)
        };
        match result {
            Ok(done) => {
                match done {
                    Some(Applied::Written) => downloaded.push(change.path.clone()),
                    Some(Applied::Removed) => removed.push(change.path.clone()),
                    None => {}
                }
                state.cursor = state.cursor.max(change.cursor);
            }
            Err(err) => {
                if apply_retry.cursor != change.cursor {
                    *apply_retry = ApplyRetry {
                        cursor: change.cursor,
                        attempts: 0,
                    };
                }
                apply_retry.attempts += 1;
                logs::append(&format!(
                    "sync: apply failed for {} ({}), attempt {}: {err}",
                    change.path, change.op, apply_retry.attempts
                ));
                if apply_retry.attempts < MAX_APPLY_ATTEMPTS {
                    complete = false;
                    break;
                }
                logs::append(&format!(
                    "sync: skipping change {} after {MAX_APPLY_ATTEMPTS} attempts",
                    change.cursor
                ));
                state.cursor = state.cursor.max(change.cursor);
            }
        }
    }
    let _ = fs::remove_dir_all(&ctx.cache_dir);
    logs::append(&format!(
        "sync: pulled {} change(s): {} written, {} removed",
        changes.len(),
        downloaded.len(),
        removed.len()
    ));
    reporter.activity("Downloaded", &downloaded);
    reporter.activity("Removed", &removed);
    Ok(complete)
}

/// This device already knows the same or a newer revision of the file.
fn is_stale(state: &SyncState, change: &RemoteChange) -> bool {
    state
        .tip_for_file_id(&change.file_id)
        .is_some_and(|(_, tip)| tip.revision >= change.revision)
}

fn is_delete(change: &RemoteChange) -> bool {
    change.op.eq_ignore_ascii_case("delete") || change.payload.deleted
}

fn is_own(ctx: &Ctx, change: &RemoteChange) -> bool {
    change.payload.updated_by_device_uuid.as_deref() == Some(ctx.device_uuid)
}

/// Our own commit coming back: the bytes are already on disk.
fn already_local(ctx: &Ctx, state: &SyncState, change: &RemoteChange) -> bool {
    is_own(ctx, change)
        && change.payload.content_sha256.is_some()
        && state
            .files
            .get(&change.path)
            .map(|t| t.content_sha256.as_str())
            == change.payload.content_sha256.as_deref()
}

fn needs_bytes(ctx: &Ctx, state: &SyncState, change: &RemoteChange) -> bool {
    !is_delete(change)
        && !is_stale(state, change)
        && !change.payload.chunk_hashes.is_empty()
        && !already_local(ctx, state, change)
}

fn fill_cache(
    ctx: &Ctx,
    hash: &str,
    local: Option<&(PathBuf, ChunkRef)>,
    url: Option<&String>,
) -> Result<(), String> {
    let dest = cache_path(&ctx.cache_dir, hash)?;
    if dest.is_file() {
        return Ok(());
    }
    let data = match local.map(|(abs, chunk)| read_chunk(abs, chunk)) {
        Some(Ok(data)) => data,
        _ => {
            let url = url.ok_or("server gave no download URL")?;
            with_retries(ctx, || ctx.store.get(url, hash))?
        }
    };
    let tmp = dest.with_extension("part");
    fs::write(&tmp, &data).map_err(|e| format!("cache write: {e}"))?;
    fs::rename(&tmp, &dest).map_err(|e| format!("cache rename: {e}"))
}

fn cache_path(cache_dir: &Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid chunk hash: {hash}"));
    }
    Ok(cache_dir.join(hash.to_ascii_lowercase()))
}

enum Applied {
    Written,
    Removed,
}

fn apply_remote_change(
    ctx: &Ctx,
    state: &mut SyncState,
    change: &RemoteChange,
) -> Result<Option<Applied>, String> {
    let root = ctx.root.as_path();
    if root.as_os_str().is_empty() {
        return Err("watch folder empty".into());
    }
    if is_stale(state, change) {
        return Ok(None);
    }

    if is_delete(change) {
        // Our own delete already removed the local file and tip.
        if is_own(ctx, change) {
            return Ok(None);
        }
        if let Some((old, _)) = state.tip_for_file_id(&change.file_id) {
            let old = old.to_string();
            remove_local_file(root, &old)?;
            state.remove_path(&old);
        }
        remove_local_file(root, &change.path)?;
        state.remove_path(&change.path);
        return Ok(Some(Applied::Removed));
    }

    // Rename: same file_id, new path — remove the old local file first.
    if let Some((old, _)) = state.tip_for_file_id(&change.file_id) {
        if old != change.path {
            let old = old.to_string();
            remove_local_file(root, &old)?;
            state.remove_path(&old);
            logs::append(&format!("sync: remote rename {old} -> {}", change.path));
        }
    }

    let content_sha = change.payload.content_sha256.clone().unwrap_or_default();
    let tip = |mtime_ns| FileTip {
        file_id: change.file_id.clone(),
        revision: change.revision,
        size: change.payload.size,
        content_sha256: content_sha.clone(),
        mtime_ns,
    };

    if already_local(ctx, state, change) {
        let mtime = file_mtime_ns(&safe_join(root, &change.path)?);
        state.upsert_tip(&change.path, tip(mtime));
        return Ok(None);
    }

    let abs = write_local_file(root, &change.path, |file| {
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        for hash in &change.payload.chunk_hashes {
            let data = fs::read(cache_path(&ctx.cache_dir, hash)?)
                .map_err(|e| format!("chunk {hash} not fetched: {e}"))?;
            hasher.update(&data);
            written += data.len() as u64;
            file.write_all(&data).map_err(|e| format!("write: {e}"))?;
        }
        if written != change.payload.size {
            return Err(format!(
                "size mismatch: got {written}, expected {}",
                change.payload.size
            ));
        }
        let got = hex::encode(hasher.finalize());
        if !content_sha.is_empty() && !got.eq_ignore_ascii_case(&content_sha) {
            return Err(format!(
                "content hash mismatch: expected {content_sha}, got {got}"
            ));
        }
        Ok(())
    })?;
    state.upsert_tip(&change.path, tip(file_mtime_ns(&abs)));
    Ok(Some(Applied::Written))
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

/// Write through a temp file next to the target, then rename into place.
fn write_local_file(
    root: &Path,
    rel: &str,
    fill: impl FnOnce(&mut fs::File) -> Result<(), String>,
) -> Result<PathBuf, String> {
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
    let written = fs::File::create(&tmp)
        .map_err(|e| format!("create temp: {e}"))
        .and_then(|mut f| {
            fill(&mut f)?;
            f.sync_all().ok();
            Ok(())
        });
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, &abs).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place: {e}")
    })?;
    Ok(abs)
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

fn file_fingerprint(path: &Path) -> Result<(u64, u64), String> {
    let meta = fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    Ok((meta.len(), mtime_ns_from_meta(&meta)))
}

fn file_mtime_ns(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .map(|meta| mtime_ns_from_meta(&meta))
        .unwrap_or(0)
}

fn mtime_ns_from_meta(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Sleep up to `total`. Returns early on stop, or on `wake` when given.
fn sleep_interruptible(stop: &AtomicBool, wake: Option<&AtomicBool>, total: Duration) {
    let mut left = total;
    let step = Duration::from_millis(100);
    while left > Duration::ZERO
        && !stop.load(Ordering::Acquire)
        && !wake.is_some_and(|w| w.load(Ordering::Acquire))
    {
        let slice = step.min(left);
        thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bst-{tag}-{nanos}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn candidate(size: u64) -> Candidate {
        Candidate {
            rel: String::new(),
            abs: PathBuf::new(),
            size,
            mtime_ns: 0,
        }
    }

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
        let root = temp_root("scan");
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
        let root = temp_root("write");
        write_local_file(&root, "x/y/z.txt", |f| {
            f.write_all(b"hello").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(fs::read(root.join("x/y/z.txt")).unwrap(), b"hello");
        remove_local_file(&root, "x/y/z.txt").unwrap();
        assert!(!root.join("x/y/z.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_write_keeps_old_file_and_no_temp() {
        let root = temp_root("write-fail");
        fs::write(root.join("a.txt"), b"old").unwrap();
        let err = write_local_file(&root, "a.txt", |f| {
            f.write_all(b"partial").unwrap();
            Err("boom".into())
        });
        assert!(err.is_err());
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_chunk_detects_changed_file() {
        let root = temp_root("read-chunk");
        let path = root.join("f.bin");
        fs::write(&path, b"hello world").unwrap();
        let chunks = chunk_file(&path).unwrap();
        assert_eq!(
            read_chunk(&path, &chunks.chunks[0]).unwrap(),
            b"hello world"
        );
        fs::write(&path, b"HELLO world").unwrap();
        assert!(read_chunk(&path, &chunks.chunks[0]).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn batches_respect_file_and_byte_caps() {
        let many: Vec<Candidate> = (0..450).map(|_| candidate(1)).collect();
        let sizes: Vec<usize> = batches(&many).iter().map(|b| b.len()).collect();
        assert_eq!(sizes, vec![200, 200, 50]);

        let big: Vec<Candidate> =
            vec![candidate(BATCH_BYTES), candidate(1), candidate(BATCH_BYTES)];
        let sizes: Vec<usize> = batches(&big).iter().map(|b| b.len()).collect();
        assert_eq!(sizes, vec![1, 1, 1]);
    }

    #[test]
    fn stale_change_is_detected_by_file_id_revision() {
        let mut state = SyncState::default();
        state.upsert_tip(
            "a.txt",
            FileTip {
                file_id: "f1".into(),
                revision: 3,
                size: 1,
                content_sha256: "aa".into(),
                mtime_ns: 1,
            },
        );
        let change = |revision| RemoteChange {
            cursor: 1,
            file_id: "f1".into(),
            path: "a.txt".into(),
            revision,
            op: "upsert".into(),
            payload: Default::default(),
        };
        assert!(is_stale(&state, &change(3)));
        assert!(!is_stale(&state, &change(4)));
    }

    /// Two-device run against a live Laravel + S3 store. Set up by
    /// `dev/e2e/two-device-sync.sh`; ignored in normal test runs.
    #[test]
    #[ignore]
    fn two_device_sync_e2e() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut reporter = Reporter::new(tx);
        let stop = AtomicBool::new(false);
        let device = |tag: &str| {
            let root = temp_root(&format!("e2e-{tag}"));
            let ctx = Ctx {
                device_uuid: Box::leak(env(&format!("BST_E2E_{tag}_UUID")).into_boxed_str()),
                root: root.clone(),
                api: SyncApiClient::new(&env("BST_E2E_API"), &env(&format!("BST_E2E_{tag}_TOKEN"))),
                store: ChunkStore::new(),
                cache_dir: root.with_extension("cache"),
                stop: &stop,
            };
            (ctx, SyncState::default())
        };
        let (a, mut sa) = device("A");
        let (b, mut sb) = device("B");
        let mut retry = ApplyRetry::default();
        let sync = |ctx: &Ctx, st: &mut SyncState, rep: &mut Reporter, retry: &mut ApplyRetry| {
            let pushed = push_local_changes(ctx, st, rep).expect("push");
            assert!(!pushed.retry, "push wanted a retry");
            pull_remote_changes(ctx, st, rep, retry).expect("pull");
        };
        let same_tree = |x: &Path, y: &Path| {
            let fx = scan_files(x).unwrap();
            let fy = scan_files(y).unwrap();
            let mut kx: Vec<_> = fx.keys().collect();
            let mut ky: Vec<_> = fy.keys().collect();
            kx.sort();
            ky.sort();
            assert_eq!(kx, ky);
            for (rel, px) in &fx {
                let hx = sha256_hex(&fs::read(px).unwrap());
                let hy = sha256_hex(&fs::read(&fy[rel]).unwrap());
                assert_eq!(hx, hy, "{rel} differs");
            }
        };
        let noisy = |len: usize, seed: u64| {
            let mut x = seed;
            (0..len)
                .map(|_| {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (x >> 33) as u8
                })
                .collect::<Vec<u8>>()
        };

        // 1. A seeds 260 files (more than one commit batch), a large file and an empty file.
        fs::create_dir_all(a.root.join("docs/deep")).unwrap();
        for i in 0..260 {
            fs::write(
                a.root.join(format!("docs/n{i:03}.txt")),
                format!("note {i}"),
            )
            .unwrap();
        }
        fs::write(a.root.join("docs/deep/big.bin"), noisy(9 * 1024 * 1024, 7)).unwrap();
        fs::write(a.root.join("empty.txt"), b"").unwrap();
        let started = std::time::Instant::now();
        sync(&a, &mut sa, &mut reporter, &mut retry);
        sync(&b, &mut sb, &mut reporter, &mut retry);
        eprintln!("seed 262 files + 9 MiB: {:?}", started.elapsed());
        same_tree(&a.root, &b.root);
        assert_eq!(sa.files.len(), 262);
        assert_eq!(sb.files.len(), 262);

        // 2. B edits the big file, deletes one note, renames another.
        let mut big = fs::read(b.root.join("docs/deep/big.bin")).unwrap();
        big[5 * 1024 * 1024] ^= 0xFF;
        fs::write(b.root.join("docs/deep/big.bin"), &big).unwrap();
        fs::remove_file(b.root.join("docs/n000.txt")).unwrap();
        fs::rename(
            b.root.join("docs/n001.txt"),
            b.root.join("docs/renamed.txt"),
        )
        .unwrap();
        sync(&b, &mut sb, &mut reporter, &mut retry);
        sync(&a, &mut sa, &mut reporter, &mut retry);
        same_tree(&a.root, &b.root);
        assert!(!a.root.join("docs/n000.txt").exists());

        // Every tip matches its file on disk, so the next scan stays cheap.
        for (tag, ctx, st) in [("A", &a, &sa), ("B", &b, &sb)] {
            for (rel, abs) in scan_files(&ctx.root).unwrap() {
                let (size, mtime) = file_fingerprint(&abs).unwrap();
                let tip = st.files.get(&rel).map(|t| (t.size, t.mtime_ns));
                assert_eq!(tip, Some((size, mtime)), "{tag} tip for {rel}");
            }
        }
        // 3. Nothing changed: a second round is a no-op on the server.
        let before = a.api.cursor().unwrap();
        sync(&a, &mut sa, &mut reporter, &mut retry);
        sync(&b, &mut sb, &mut reporter, &mut retry);
        let extra: Vec<String> = a
            .api
            .changes(before)
            .unwrap()
            .changes
            .iter()
            .map(|c| {
                format!(
                    "{} {} rev{} by {:?}",
                    c.op, c.path, c.revision, c.payload.updated_by_device_uuid
                )
            })
            .collect();
        assert!(extra.is_empty(), "no-op round committed: {extra:?}");

        // 4. Both edit the same file; the later commit (B) wins on both sides.
        fs::write(a.root.join("empty.txt"), b"from A").unwrap();
        fs::write(b.root.join("empty.txt"), b"from B").unwrap();
        push_local_changes(&a, &mut sa, &mut reporter).unwrap();
        sync(&b, &mut sb, &mut reporter, &mut retry);
        sync(&a, &mut sa, &mut reporter, &mut retry);
        same_tree(&a.root, &b.root);
        assert_eq!(fs::read(a.root.join("empty.txt")).unwrap(), b"from B");

        let _ = fs::remove_dir_all(&a.root);
        let _ = fs::remove_dir_all(&b.root);
    }
}
