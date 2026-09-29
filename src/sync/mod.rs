//! In-process sync engine. The bucket is the file index.
//!
//! Each round lists the customer folder (one call), compares it and the local
//! folder with what this device last synced, and moves only what differs:
//! parallel streaming PUT/GET/DELETE on presigned URLs that Laravel signs.
//! The server keeps no file records, so nothing can drift from the bucket.
//!
//! Files are stored whole under their real name and path. The device holds no
//! store keys.

mod client;
mod pool;
mod state;
mod store;
mod watch;

use crate::app::AppCommand;
use crate::config::{self, Config};
use crate::logs;
use client::{ApiError, Method, RemoteFile, SyncApiClient};
use state::{state_path, SyncState, Synced};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};
use store::{hash_file, FileStore, MAX_FILE_BYTES};
use watch::FolderWatcher;

/// Temp files (downloads in progress) end with this.
const TEMP_SUFFIX: &str = ".bst-tmp";

/// OS files that never sync: Finder, Spotlight, Explorer and Office lock files.
const IGNORED_NAMES: &[&str] = &[
    ".DS_Store",
    ".localized",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    ".TemporaryItems",
    "Icon\r",
    "Thumbs.db",
    "desktop.ini",
    "$RECYCLE.BIN",
];

/// True for our temp files and OS junk. Scan, watcher and pull all skip them.
fn is_ignored(name: &str) -> bool {
    name.ends_with(TEMP_SUFFIX)
        || name.starts_with("._")
        || name.starts_with("~$")
        || IGNORED_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// True when any segment of a relative path is ignored.
fn is_ignored_path(rel: &str) -> bool {
    rel.split('/').any(is_ignored)
}

/// How often the engine lists the bucket when idle.
const REMOTE_POLL: Duration = Duration::from_secs(4);
const ERROR_BACKOFF: Duration = Duration::from_secs(5);
/// Let writes settle after a file-system event before scanning.
const FS_SETTLE: Duration = Duration::from_millis(500);
const TRANSFER_WORKERS: usize = 8;
const HASH_WORKERS: usize = 4;
/// Files per signing request (server cap is 500).
const BATCH_FILES: usize = 200;
/// Soft cap on bytes per batch. URLs are signed per batch, just before use,
/// so they stay fresh on slow links.
const BATCH_BYTES: u64 = 256 * 1024 * 1024;
/// Extra attempts for one file PUT/GET before it counts as failed.
const TRANSFER_RETRIES: u32 = 2;
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
            .name("file-sync".into())
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
    root: PathBuf,
    api: SyncApiClient,
    store: FileStore,
    /// Paths already logged as too large, so scans do not repeat the line.
    oversize_logged: Mutex<HashSet<String>>,
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

fn run_loop(cfg: Config, stop: Arc<AtomicBool>, mut reporter: Reporter) {
    let device_token = match crate::secret::decrypt(&cfg.device_token_enc) {
        Ok(v) => v,
        Err(err) => {
            logs::append(&format!("sync: device token decrypt failed: {err}"));
            reporter.failed("Stored credentials could not be read. Pair this computer again.");
            return;
        }
    };

    let state_path = state_path(&cfg.device_uuid);
    let ctx = Ctx {
        root: PathBuf::from(cfg.watch_folder.trim()),
        api: SyncApiClient::new(&cfg.pair_api_base, &device_token),
        store: FileStore::new(),
        oversize_logged: Mutex::new(HashSet::new()),
        stop: &stop,
    };

    let mut state = SyncState::load(&state_path);
    let dirty = Arc::new(AtomicBool::new(true));
    let _watcher = FolderWatcher::start(&ctx.root, Arc::clone(&dirty));

    while !ctx.stopping() {
        if dirty.load(Ordering::Acquire) {
            sleep_interruptible(&stop, None, FS_SETTLE);
        }
        // A file-system event forces a local scan; a quiet poll scans only
        // when the bucket listing differs from the last sync.
        let scan = dirty.swap(false, Ordering::AcqRel);
        match sync_round(&ctx, &mut state, scan, &mut reporter) {
            Ok(outcome) => {
                if outcome.changed {
                    if let Err(err) = state.save(&state_path) {
                        logs::append(&format!("sync: state save failed: {err}"));
                    }
                }
                reporter.status(true, "idle", state.files.len(), 0, 0);
                if outcome.retry {
                    dirty.store(true, Ordering::Release);
                }
            }
            Err(ApiError::Auth(err)) => {
                logs::append(&format!("sync: auth error: {err}"));
                let _ = state.save(&state_path);
                reporter.failed("This computer was disconnected. Pair it again.");
                return;
            }
            Err(err) => {
                logs::append(&format!("sync: round failed: {err}"));
                reporter.status(false, "offline", state.files.len(), 0, 0);
                dirty.store(true, Ordering::Release);
                sleep_interruptible(&stop, None, ERROR_BACKOFF);
                continue;
            }
        }
        // Wake at once on FS events; otherwise list the bucket again.
        sleep_interruptible(&stop, Some(&dirty), REMOTE_POLL);
    }
    let _ = state.save(&state_path);
}

/// Local file facts taken during the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Local {
    size: u64,
    mtime_ns: u64,
}

/// What to do with one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Skip,
    Upload,
    Download,
    DeleteRemote,
    DeleteLocal,
    /// Neither side has the file any more.
    Forget,
    /// Both sides have it: hash the local file. Equal bytes just update the
    /// state; different bytes take `if_different`.
    Compare(Different),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Different {
    /// Only the local side changed.
    Upload,
    /// Both sides changed (or the file is new to this device): newer wins.
    NewerWins,
}

/// Three-way compare of one path: local file, bucket object and the state
/// from the last sync. `keep_local` stops "gone from the bucket" from
/// deleting local files (an empty listing that follows a full one).
fn decide(
    tip: Option<&Synced>,
    local: Option<Local>,
    remote: Option<&RemoteFile>,
    keep_local: bool,
) -> Decision {
    let local_changed = |t: &Synced, l: Local| (l.size, l.mtime_ns) != (t.size, t.mtime_ns);
    match (local, remote, tip) {
        (Some(_), Some(_), None) => Decision::Compare(Different::NewerWins),
        (Some(l), Some(r), Some(t)) => match (local_changed(t, l), r.etag != t.etag) {
            (false, false) => Decision::Skip,
            (false, true) => Decision::Download,
            (true, false) => Decision::Compare(Different::Upload),
            (true, true) => Decision::Compare(Different::NewerWins),
        },
        (Some(_), None, None) => Decision::Upload,
        (Some(l), None, Some(t)) => {
            if local_changed(t, l) || keep_local {
                Decision::Upload
            } else {
                Decision::DeleteLocal
            }
        }
        (None, Some(_), None) => Decision::Download,
        (None, Some(r), Some(t)) => {
            if r.etag == t.etag {
                Decision::DeleteRemote
            } else {
                Decision::Download
            }
        }
        (None, None, Some(_)) => Decision::Forget,
        (None, None, None) => Decision::Skip,
    }
}

/// True when the listing differs from the state: a quiet round then has
/// something to do even without a local change.
fn remote_differs(remote: &HashMap<String, RemoteFile>, state: &SyncState) -> bool {
    remote.len() != state.files.len()
        || remote
            .iter()
            .any(|(path, file)| state.files.get(path).map(|t| &t.etag) != Some(&file.etag))
}

struct Upload {
    rel: String,
    abs: PathBuf,
    local: Local,
}

struct Download {
    rel: String,
    remote: RemoteFile,
    /// Local file seen at plan time. A change before the rename cancels it.
    local: Option<Local>,
}

#[derive(Default)]
struct Plan {
    uploads: Vec<Upload>,
    downloads: Vec<Download>,
    remote_deletes: Vec<String>,
    local_deletes: Vec<String>,
}

fn sync_round(
    ctx: &Ctx,
    state: &mut SyncState,
    scan: bool,
    reporter: &mut Reporter,
) -> Result<Outcome, ApiError> {
    let mut out = Outcome::default();
    if !ctx.root.is_dir() {
        return Ok(out);
    }
    let remote: HashMap<String, RemoteFile> = ctx
        .api
        .list_files()?
        .into_iter()
        .filter(|f| !f.path.ends_with('/') && !is_ignored_path(&f.path))
        .map(|f| (f.path.clone(), f))
        .collect();
    if !scan && !remote_differs(&remote, state) {
        return Ok(out);
    }

    let files = scan_files(&ctx.root).map_err(ApiError::Other)?;
    let plan = make_plan(ctx, state, &files, &remote, &mut out);
    let mut need_files = (plan.uploads.len()
        + plan.downloads.len()
        + plan.remote_deletes.len()
        + plan.local_deletes.len()) as u64;
    if need_files == 0 {
        return Ok(out);
    }
    let mut need_bytes = plan.uploads.iter().map(|u| u.local.size).sum::<u64>()
        + plan.downloads.iter().map(|d| d.remote.size).sum::<u64>();
    reporter.status(true, "syncing", state.files.len(), need_files, need_bytes);

    let mut progress = |reporter: &mut Reporter, state: &SyncState, files: usize, bytes: u64| {
        need_files = need_files.saturating_sub(files as u64);
        need_bytes = need_bytes.saturating_sub(bytes);
        reporter.status(true, "syncing", state.files.len(), need_files, need_bytes);
    };

    let sizes: Vec<u64> = plan.uploads.iter().map(|u| u.local.size).collect();
    for range in chunks(&sizes) {
        if ctx.stopping() {
            return Ok(out);
        }
        let batch = &plan.uploads[range];
        upload_batch(ctx, state, batch, reporter, &mut out)?;
        progress(
            reporter,
            state,
            batch.len(),
            batch.iter().map(|u| u.local.size).sum(),
        );
    }

    let sizes: Vec<u64> = plan.downloads.iter().map(|d| d.remote.size).collect();
    for range in chunks(&sizes) {
        if ctx.stopping() {
            return Ok(out);
        }
        let batch = &plan.downloads[range];
        download_batch(ctx, state, batch, reporter, &mut out)?;
        progress(
            reporter,
            state,
            batch.len(),
            batch.iter().map(|d| d.remote.size).sum(),
        );
    }

    for group in plan.remote_deletes.chunks(BATCH_FILES) {
        if ctx.stopping() {
            return Ok(out);
        }
        delete_remote_batch(ctx, state, group, reporter, &mut out)?;
        progress(reporter, state, group.len(), 0);
    }

    let mut removed = Vec::new();
    for rel in &plan.local_deletes {
        match remove_local_file(&ctx.root, rel) {
            Ok(()) => {
                state.files.remove(rel);
                removed.push(rel.clone());
                out.changed = true;
            }
            Err(err) => {
                logs::append(&format!("sync: remove {rel} failed: {err}"));
                out.retry = true;
            }
        }
    }
    reporter.activity("Removed", &removed);
    Ok(out)
}

/// Decide every path. Paths that only need a state update are applied here.
fn make_plan(
    ctx: &Ctx,
    state: &mut SyncState,
    files: &HashMap<String, PathBuf>,
    remote: &HashMap<String, RemoteFile>,
    out: &mut Outcome,
) -> Plan {
    let mut paths: Vec<String> = files
        .keys()
        .chain(remote.keys())
        .chain(state.files.keys())
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    paths.sort();
    let keep_local = remote.is_empty() && !state.files.is_empty();

    let mut plan = Plan::default();
    let mut to_compare = Vec::new();
    for rel in &paths {
        let abs = files.get(rel);
        let local = match abs.map(|abs| file_fingerprint(abs)) {
            None => None,
            Some(Ok((size, _))) if size > MAX_FILE_BYTES => {
                let first = ctx
                    .oversize_logged
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(rel.clone());
                if first {
                    logs::append(&format!("sync: skip {rel}: larger than 5 GiB"));
                }
                continue;
            }
            Some(Ok((size, mtime_ns))) => Some(Local { size, mtime_ns }),
            Some(Err(_)) => {
                out.retry = true;
                continue;
            }
        };
        let tip = state.files.get(rel);
        match decide(tip, local, remote.get(rel), keep_local) {
            Decision::Skip => {}
            Decision::Upload => plan.uploads.push(Upload {
                rel: rel.clone(),
                abs: abs.cloned().unwrap_or_default(),
                local: local.unwrap_or(Local {
                    size: 0,
                    mtime_ns: 0,
                }),
            }),
            Decision::Download => plan.downloads.push(Download {
                rel: rel.clone(),
                remote: remote[rel].clone(),
                local,
            }),
            Decision::DeleteRemote => plan.remote_deletes.push(rel.clone()),
            Decision::DeleteLocal => plan.local_deletes.push(rel.clone()),
            Decision::Forget => {
                state.files.remove(rel);
                out.changed = true;
            }
            Decision::Compare(different) => to_compare.push((rel, different)),
        }
    }

    // Hash the files that exist on both sides, in parallel.
    let hashes = pool::parallel_map(&to_compare, HASH_WORKERS, |(rel, _)| {
        if ctx.stopping() {
            return Err("stopped".to_string());
        }
        hash_file(&files[*rel]).map(|(md5, _)| md5)
    });
    for ((rel, different), hash) in to_compare.into_iter().zip(hashes) {
        let Ok(md5) = hash else {
            out.retry = true;
            continue;
        };
        let local = file_fingerprint(&files[rel]).map(|(size, mtime_ns)| Local { size, mtime_ns });
        let Ok(local) = local else {
            out.retry = true;
            continue;
        };
        let remote = &remote[rel];
        if md5.eq_ignore_ascii_case(&remote.etag) {
            // Same bytes on both sides: only remember it.
            state.files.insert(
                rel.clone(),
                Synced {
                    etag: remote.etag.clone(),
                    size: local.size,
                    mtime_ns: local.mtime_ns,
                },
            );
            out.changed = true;
            continue;
        }
        let upload = match different {
            Different::Upload => true,
            // Last writer wins: the file with the later mtime.
            Different::NewerWins => parse_rfc3339_ns(&remote.modified)
                .is_none_or(|remote_ns| local.mtime_ns > remote_ns),
        };
        if upload {
            plan.uploads.push(Upload {
                rel: rel.clone(),
                abs: files[rel].clone(),
                local,
            });
        } else {
            plan.downloads.push(Download {
                rel: rel.clone(),
                remote: remote.clone(),
                local: Some(local),
            });
        }
    }
    plan
}

/// Split work into batches bounded by file count and bytes.
fn chunks(sizes: &[u64]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bytes = 0u64;
    for (i, size) in sizes.iter().enumerate() {
        let full = i - start >= BATCH_FILES || (i > start && bytes + size > BATCH_BYTES);
        if full {
            out.push(start..i);
            start = i;
            bytes = 0;
        }
        bytes += size;
    }
    if start < sizes.len() {
        out.push(start..sizes.len());
    }
    out
}

fn upload_batch(
    ctx: &Ctx,
    state: &mut SyncState,
    batch: &[Upload],
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    let paths: Vec<String> = batch.iter().map(|u| u.rel.clone()).collect();
    let urls = ctx.api.sign(Method::Put, &paths)?;
    let jobs: Vec<(&Upload, &String)> = batch.iter().zip(&urls).collect();
    let results = pool::parallel_map(&jobs, TRANSFER_WORKERS, |(u, url)| {
        if ctx.stopping() {
            return Err("stopped".to_string());
        }
        let etag = with_retries(ctx, || ctx.store.put_file(url, &u.abs))?;
        // Stored bytes must be the bytes that were planned.
        match file_fingerprint(&u.abs) {
            Ok((size, mtime_ns)) if (size, mtime_ns) == (u.local.size, u.local.mtime_ns) => {
                Ok(etag)
            }
            _ => Err("changed while syncing".to_string()),
        }
    });
    let mut done = Vec::new();
    for ((u, _), result) in jobs.iter().zip(results) {
        match result {
            Ok(etag) => {
                state.files.insert(
                    u.rel.clone(),
                    Synced {
                        etag,
                        size: u.local.size,
                        mtime_ns: u.local.mtime_ns,
                    },
                );
                done.push(u.rel.clone());
            }
            Err(err) => {
                logs::append(&format!("sync: upload {} failed: {err}", u.rel));
                out.retry = true;
            }
        }
    }
    out.changed |= !done.is_empty();
    logs::append(&format!("sync: uploaded {} file(s)", done.len()));
    reporter.activity("Uploaded", &done);
    Ok(())
}

fn download_batch(
    ctx: &Ctx,
    state: &mut SyncState,
    batch: &[Download],
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    // Temp file next to the target, so the final rename is atomic.
    let mut ready: Vec<(&Download, PathBuf, PathBuf)> = Vec::new();
    for d in batch {
        let prepared = safe_join(&ctx.root, &d.rel).and_then(|abs| {
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            let tmp = temp_path_for(&abs);
            Ok((abs, tmp))
        });
        match prepared {
            Ok((abs, tmp)) => ready.push((d, abs, tmp)),
            Err(err) => {
                logs::append(&format!("sync: skip {}: {err}", d.rel));
                out.retry = true;
            }
        }
    }
    if ready.is_empty() {
        return Ok(());
    }

    let paths: Vec<String> = ready.iter().map(|(d, _, _)| d.rel.clone()).collect();
    let urls = ctx.api.sign(Method::Get, &paths)?;
    let jobs: Vec<_> = ready.iter().zip(&urls).collect();
    let results = pool::parallel_map(&jobs, TRANSFER_WORKERS, |((d, _, tmp), url)| {
        if ctx.stopping() {
            return Err("stopped".to_string());
        }
        // Only a plain MD5 ETag proves the bytes; multipart ETags do not.
        let md5 = if is_md5_etag(&d.remote.etag) {
            d.remote.etag.as_str()
        } else {
            ""
        };
        with_retries(ctx, || ctx.store.get_file(url, tmp, d.remote.size, md5))
    });

    let mut done = Vec::new();
    for (((d, abs, tmp), _), result) in jobs.iter().zip(results) {
        if let Err(err) = result {
            logs::append(&format!("sync: download {} failed: {err}", d.rel));
            out.retry = true;
            continue;
        }
        // The user edited the file while it downloaded: keep their edit.
        let now = file_fingerprint(abs)
            .ok()
            .map(|(size, mtime_ns)| Local { size, mtime_ns });
        if now != d.local {
            let _ = fs::remove_file(tmp);
            out.retry = true;
            continue;
        }
        if let Err(err) = fs::rename(tmp, abs) {
            let _ = fs::remove_file(tmp);
            logs::append(&format!("sync: place {} failed: {err}", d.rel));
            out.retry = true;
            continue;
        }
        state.files.insert(
            d.rel.clone(),
            Synced {
                etag: d.remote.etag.clone(),
                size: d.remote.size,
                mtime_ns: file_mtime_ns(abs),
            },
        );
        done.push(d.rel.clone());
    }
    out.changed |= !done.is_empty();
    logs::append(&format!("sync: downloaded {} file(s)", done.len()));
    reporter.activity("Downloaded", &done);
    Ok(())
}

fn delete_remote_batch(
    ctx: &Ctx,
    state: &mut SyncState,
    batch: &[String],
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    let urls = ctx.api.sign(Method::Delete, batch)?;
    let jobs: Vec<(&String, &String)> = batch.iter().zip(&urls).collect();
    let results = pool::parallel_map(&jobs, TRANSFER_WORKERS, |(_, url)| {
        with_retries(ctx, || ctx.store.delete(url))
    });
    let mut done = Vec::new();
    for ((rel, _), result) in jobs.iter().zip(results) {
        match result {
            Ok(()) => {
                state.files.remove(*rel);
                done.push((*rel).clone());
            }
            Err(err) => {
                logs::append(&format!("sync: delete {rel} failed: {err}"));
                out.retry = true;
            }
        }
    }
    out.changed |= !done.is_empty();
    logs::append(&format!(
        "sync: deleted {} file(s) from the bucket",
        done.len()
    ));
    reporter.activity("Deleted", &done);
    Ok(())
}

/// A single-part ETag is the 32-hex MD5 of the bytes.
fn is_md5_etag(etag: &str) -> bool {
    etag.len() == 32 && etag.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Download temp file: same folder as the target, and a name the scanner and
/// watcher ignore.
fn temp_path_for(abs: &Path) -> PathBuf {
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    abs.with_file_name(format!(".{name}{TEMP_SUFFIX}"))
}

/// Nanoseconds since the UNIX epoch for `2026-01-02T03:04:05[.fff]Z`.
fn parse_rfc3339_ns(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut t = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    let mut nanos = 0i64;
    for (i, digit) in fraction.chars().take(9).enumerate() {
        nanos += i64::from(digit.to_digit(10)?) * 10i64.pow(8 - i as u32);
    }
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + min * 60 + s;
    u64::try_from(secs * 1_000_000_000 + nanos).ok()
}

/// Retry a transfer in place: stores return 500/503 and drop connections
/// now and then, and one bad file should not fail a batch.
fn with_retries<T>(ctx: &Ctx, mut op: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut attempt = 0u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(_) if attempt < TRANSFER_RETRIES && !ctx.stopping() => {
                attempt += 1;
                thread::sleep(Duration::from_millis(250 << attempt));
            }
            Err(err) => return Err(err),
        }
    }
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
            if is_ignored(&name) {
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

    fn tip(etag: &str) -> Synced {
        Synced {
            etag: etag.into(),
            size: 5,
            mtime_ns: 100,
        }
    }

    fn local(mtime_ns: u64) -> Option<Local> {
        Some(Local { size: 5, mtime_ns })
    }

    fn remote(etag: &str) -> RemoteFile {
        RemoteFile {
            path: "a.txt".into(),
            size: 5,
            etag: etag.into(),
            modified: "2026-01-02T03:04:05.000Z".into(),
        }
    }

    #[test]
    fn decide_compares_three_ways() {
        let t = tip("e1");
        let same = remote("e1");
        let newer = remote("e2");
        // Nothing changed.
        assert_eq!(
            decide(Some(&t), local(100), Some(&same), false),
            Decision::Skip
        );
        // Only the bucket changed.
        assert_eq!(
            decide(Some(&t), local(100), Some(&newer), false),
            Decision::Download
        );
        // Only the local file changed: upload unless the bytes are equal.
        assert_eq!(
            decide(Some(&t), local(200), Some(&same), false),
            Decision::Compare(Different::Upload)
        );
        // Both changed, or a file this device never synced: newer wins.
        assert_eq!(
            decide(Some(&t), local(200), Some(&newer), false),
            Decision::Compare(Different::NewerWins)
        );
        assert_eq!(
            decide(None, local(100), Some(&same), false),
            Decision::Compare(Different::NewerWins)
        );
        // New on one side only.
        assert_eq!(decide(None, local(100), None, false), Decision::Upload);
        assert_eq!(decide(None, None, Some(&same), false), Decision::Download);
        // Deleted locally: delete in the bucket only if nobody changed it.
        assert_eq!(
            decide(Some(&t), None, Some(&same), false),
            Decision::DeleteRemote
        );
        assert_eq!(
            decide(Some(&t), None, Some(&newer), false),
            Decision::Download
        );
        // Deleted in the bucket: delete locally only if the local file is untouched.
        assert_eq!(
            decide(Some(&t), local(100), None, false),
            Decision::DeleteLocal
        );
        assert_eq!(decide(Some(&t), local(200), None, false), Decision::Upload);
        // An empty listing after a full one never wipes the folder.
        assert_eq!(decide(Some(&t), local(100), None, true), Decision::Upload);
        assert_eq!(decide(Some(&t), None, None, false), Decision::Forget);
    }

    #[test]
    fn quiet_round_needs_a_listing_change() {
        let mut state = SyncState::default();
        state.files.insert("a.txt".into(), tip("e1"));
        let mut remote_map = HashMap::from([("a.txt".to_string(), remote("e1"))]);
        assert!(!remote_differs(&remote_map, &state));
        remote_map.insert("a.txt".into(), remote("e2"));
        assert!(remote_differs(&remote_map, &state));
        remote_map.clear();
        assert!(remote_differs(&remote_map, &state));
    }

    #[test]
    fn parses_bucket_timestamps() {
        assert_eq!(parse_rfc3339_ns("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ns("2026-01-02T03:04:05.250Z"),
            Some(1_767_323_045_250_000_000)
        );
        assert_eq!(
            parse_rfc3339_ns("2024-03-01T00:00:00Z"),
            Some(1_709_251_200_000_000_000)
        );
        assert_eq!(parse_rfc3339_ns("nonsense"), None);
    }

    #[test]
    fn only_plain_md5_etags_are_checked() {
        assert!(is_md5_etag("d41d8cd98f00b204e9800998ecf8427e"));
        assert!(!is_md5_etag("d41d8cd98f00b204e9800998ecf8427e-3"));
        assert!(!is_md5_etag(""));
    }

    #[test]
    fn chunks_respect_file_and_byte_caps() {
        let many = vec![1u64; 450];
        let sizes: Vec<usize> = chunks(&many).iter().map(|r| r.len()).collect();
        assert_eq!(sizes, vec![200, 200, 50]);
        let big = [BATCH_BYTES, 1, BATCH_BYTES];
        assert_eq!(chunks(&big), vec![0..1, 1..2, 2..3]);
        assert!(chunks(&[]).is_empty());
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
    fn scan_ignores_download_temp_files() {
        let root = temp_root("scan-tmp");
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(temp_path_for(&root.join("a.txt")), b"partial").unwrap();
        let files = scan_files(&root).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), vec!["a.txt"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_ignores_os_junk() {
        let root = temp_root("scan-junk");
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(root.join(".DS_Store"), b"x").unwrap();
        fs::write(root.join("._a.txt"), b"x").unwrap();
        fs::write(root.join("~$report.docx"), b"x").unwrap();
        fs::create_dir_all(root.join(".Trashes")).unwrap();
        fs::write(root.join(".Trashes/b.txt"), b"x").unwrap();
        let files = scan_files(&root).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), vec!["a.txt"]);
        assert!(is_ignored_path("docs/.DS_Store"));
        assert!(!is_ignored_path("docs/a.txt"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn temp_path_is_next_to_target() {
        let abs = Path::new("/w/docs/a.txt");
        let tmp = temp_path_for(abs);
        assert_eq!(tmp.parent(), abs.parent());
        assert!(is_ignored(&tmp.file_name().unwrap().to_string_lossy()));
    }

    #[test]
    fn hash_file_streams_and_matches_known_digest() {
        let root = temp_root("hash");
        let path = root.join("f.txt");
        fs::write(&path, b"abc").unwrap();
        let (md5, size) = hash_file(&path).unwrap();
        assert_eq!(md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(size, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remove_nested_file() {
        let root = temp_root("remove");
        fs::create_dir_all(root.join("x/y")).unwrap();
        fs::write(root.join("x/y/z.txt"), b"hello").unwrap();
        remove_local_file(&root, "x/y/z.txt").unwrap();
        assert!(!root.join("x/y/z.txt").exists());
        // Removing a missing file is fine.
        remove_local_file(&root, "x/y/z.txt").unwrap();
        let _ = fs::remove_dir_all(root);
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
                root,
                api: SyncApiClient::new(&env("BST_E2E_API"), &env(&format!("BST_E2E_{tag}_TOKEN"))),
                store: FileStore::new(),
                oversize_logged: Mutex::new(HashSet::new()),
                stop: &stop,
            };
            (ctx, SyncState::default())
        };
        let (a, mut sa) = device("A");
        let (b, mut sb) = device("B");
        let mut sync = |ctx: &Ctx, st: &mut SyncState| {
            let out = sync_round(ctx, st, true, &mut reporter).expect("sync");
            assert!(!out.retry, "sync wanted a retry");
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
                let hx = hash_file(px).unwrap().0;
                let hy = hash_file(&fy[rel]).unwrap().0;
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

        // 1. A seeds 260 files (more than one batch), a large file and an empty file.
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
        sync(&a, &mut sa);
        sync(&b, &mut sb);
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
        sync(&b, &mut sb);
        sync(&a, &mut sa);
        same_tree(&a.root, &b.root);
        assert!(!a.root.join("docs/n000.txt").exists());
        assert!(a.root.join("docs/renamed.txt").exists());

        // The state matches the disk and the bucket, so a quiet round does nothing.
        let listed = a.api.list_files().unwrap();
        for (tag, ctx, st) in [("A", &a, &sa), ("B", &b, &sb)] {
            assert_eq!(st.files.len(), listed.len(), "{tag} state size");
            for (rel, abs) in scan_files(&ctx.root).unwrap() {
                let (size, mtime) = file_fingerprint(&abs).unwrap();
                let synced = &st.files[&rel];
                assert_eq!((synced.size, synced.mtime_ns), (size, mtime), "{tag} {rel}");
                let remote = listed.iter().find(|f| f.path == rel).unwrap();
                assert_eq!(synced.etag, remote.etag, "{tag} {rel} etag");
            }
        }
        // No download temp file is left behind on either side.
        for ctx in [&a, &b] {
            let mut stack = vec![ctx.root.clone()];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(dir).unwrap() {
                    let path = entry.unwrap().path();
                    assert!(!path.to_string_lossy().ends_with(TEMP_SUFFIX), "{path:?}");
                    if path.is_dir() {
                        stack.push(path);
                    }
                }
            }
        }
        let before: Vec<_> = listed
            .iter()
            .map(|f| (f.path.clone(), f.etag.clone()))
            .collect();
        sync(&a, &mut sa);
        sync(&b, &mut sb);
        let after: Vec<_> = a
            .api
            .list_files()
            .unwrap()
            .iter()
            .map(|f| (f.path.clone(), f.etag.clone()))
            .collect();
        assert_eq!(before, after, "no-op round changed the bucket");

        // 3. Both edit the same file; the write that lands later (B's) wins on both sides.
        fs::write(a.root.join("empty.txt"), b"from A").unwrap();
        sync(&a, &mut sa);
        std::thread::sleep(Duration::from_millis(1500));
        fs::write(b.root.join("empty.txt"), b"from B").unwrap();
        sync(&b, &mut sb);
        sync(&a, &mut sa);
        same_tree(&a.root, &b.root);
        assert_eq!(fs::read(a.root.join("empty.txt")).unwrap(), b"from B");

        // 4. A file put straight into the bucket lands on both devices.
        let url = a
            .api
            .sign(Method::Put, &["manual/by-hand.txt".to_string()])
            .unwrap();
        a.store
            .put_file(&url[0], &{
                let p = a
                    .root
                    .join("..")
                    .join(format!("hand-{}.txt", std::process::id()));
                fs::write(&p, b"by hand").unwrap();
                p
            })
            .unwrap();
        sync(&b, &mut sb);
        assert_eq!(
            fs::read(b.root.join("manual/by-hand.txt")).unwrap(),
            b"by hand"
        );

        // 5. Delete on A removes the file from the bucket and from B.
        fs::remove_file(a.root.join("docs/n002.txt")).unwrap();
        sync(&a, &mut sa);
        sync(&b, &mut sb);
        assert!(!b.root.join("docs/n002.txt").exists());

        let _ = fs::remove_dir_all(&a.root);
        let _ = fs::remove_dir_all(&b.root);
    }
}
