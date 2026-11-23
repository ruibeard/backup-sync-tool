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
mod ignore;
mod pool;
mod state;
mod store;
mod watch;

use crate::app::{AppCommand, Transfer};
use crate::config::{self, Config};
use crate::logs;
use client::{ApiError, Method, MoveRequest, RemoteFile, SyncApiClient};
use state::{state_path, SyncState, Synced};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, UNIX_EPOCH};
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
/// Files below `BIG_FILE_BYTES` (photos, documents) run 32 at a time:
/// they spend most of their time waiting on the server, not on bandwidth.
const SMALL_WORKERS: usize = 32;
/// Big files (videos, models, archives) run 8 at a time; each one already
/// downloads in parallel parts.
const BIG_WORKERS: usize = 8;
const BIG_FILE_BYTES: u64 = store::RANGED_MIN_BYTES;
const HASH_WORKERS: usize = 4;
/// Files per signing request (server cap is 500).
const BATCH_FILES: usize = 200;
/// Soft cap on bytes per batch. URLs are signed per batch, just before use,
/// so they stay fresh on slow links.
const BATCH_BYTES: u64 = 256 * 1024 * 1024;
/// One `sync/move` call: the server copies these inside the store. The
/// byte cap keeps each call well under proxy timeouts.
const MOVE_BATCH_FILES: usize = 100;
const MOVE_BATCH_BYTES: u64 = 1024 * 1024 * 1024;
/// New files from this size up are first looked up in the bucket history
/// by MD5: a file moved or put back while the app was off is copied there,
/// not uploaded. Smaller files upload faster than the lookup.
const REUSE_MIN_BYTES: u64 = 256 * 1024;
/// Extra attempts for one file PUT/GET before it counts as failed.
const TRANSFER_RETRIES: u32 = 2;
/// How often the UI gets live progress while files move.
const PROGRESS_TICK: Duration = Duration::from_secs(1);
/// Speed is the average over this window.
const RATE_WINDOW: Duration = Duration::from_secs(5);
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
    last: Option<(bool, &'static str, u64, u64, u64, Transfer)>,
}

impl Reporter {
    fn new(tx: Sender<AppCommand>) -> Self {
        Self { tx, last: None }
    }

    /// True when the window last showed "All synced".
    fn shows_idle(&self) -> bool {
        matches!(self.last, Some((true, "idle", ..)))
    }

    fn status(
        &mut self,
        connected: bool,
        folder_state: &'static str,
        local: usize,
        need_files: u64,
        need_bytes: u64,
        transfer: Transfer,
    ) {
        let next = (
            connected,
            folder_state,
            local as u64,
            need_files,
            need_bytes,
            transfer,
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
            transfer,
        });
    }

    /// One activity line. Safe to call from transfer workers.
    fn line(&self, text: String) {
        let _ = self.tx.send(AppCommand::Activity(text));
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
    /// Files finished in the current round (for live progress).
    files_done: AtomicU64,
    /// Planned upload bytes a server-side copy made unnecessary this round.
    up_skipped: AtomicU64,
    /// New paths whose server-side move failed or found no stored copy:
    /// they upload, and are not asked again until the app restarts.
    move_failed: Mutex<HashSet<String>>,
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
        store: FileStore::new().with_stop(Arc::clone(&stop)),
        oversize_logged: Mutex::new(HashSet::new()),
        stop: &stop,
        files_done: AtomicU64::new(0),
        up_skipped: AtomicU64::new(0),
        move_failed: Mutex::new(HashSet::new()),
    };

    // No download runs yet: temp files here are left from a killed app.
    remove_stale_temps(&ctx.root);
    let mut state = SyncState::load_for(&state_path, &ctx.root.to_string_lossy());
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
                reporter.status(true, "idle", state.files.len(), 0, 0, Transfer::default());
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
                reporter.status(
                    false,
                    "offline",
                    state.files.len(),
                    0,
                    0,
                    Transfer::default(),
                );
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

/// This device renamed `from` to `upload.rel`: the server copies the object
/// instead of the device uploading the bytes again. With no `from`, the
/// server copies any stored version with the same MD5, if it has one.
struct RemoteMove {
    from: Option<String>,
    /// The bucket ETag of `from`; the bytes of the new path have this MD5.
    etag: String,
    upload: Upload,
}

/// A new bucket file that this device already holds at `from` with the same
/// bytes: rename or copy it on disk instead of downloading it.
struct LocalMove {
    from: String,
    from_local: Local,
    /// Rename when `from` is gone from the bucket; copy when it stays.
    rename: bool,
    download: Download,
}

#[derive(Default)]
struct Plan {
    uploads: Vec<Upload>,
    downloads: Vec<Download>,
    remote_deletes: Vec<String>,
    local_deletes: Vec<String>,
    remote_moves: Vec<RemoteMove>,
    local_moves: Vec<LocalMove>,
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
    let ignore = ignore::Ignore::load(&ctx.root);
    // Ignored paths drop out of the state too, so they are never deleted.
    let before = state.files.len();
    state.files.retain(|rel, _| !ignore.matches(rel));
    out.changed |= state.files.len() != before;
    let remote: HashMap<String, RemoteFile> = ctx
        .api
        .list_files()?
        .into_iter()
        .filter(|f| !f.path.ends_with('/') && !is_ignored_path(&f.path) && !ignore.matches(&f.path))
        .map(|f| (f.path.clone(), f))
        .collect();
    if !scan && !remote_differs(&remote, state) {
        return Ok(out);
    }
    // The listing worked: the server is reachable. Say so now, not after
    // the scan and the hashing, which can take minutes on a big folder.
    // When the window already shows "All synced", keep it: a re-scan after
    // a bucket change or our own download finds nothing most times. The
    // hashing and the transfers below show their own progress.
    let local = state.files.len();
    if !reporter.shows_idle() {
        reporter.status(true, "scanning", local, 0, 0, Transfer::default());
    }

    let mut files = scan_files(&ctx.root).map_err(ApiError::Other)?;
    files.retain(|rel, _| !ignore.matches(rel));
    let mut plan = make_plan(
        ctx,
        state,
        &files,
        &remote,
        &mut out,
        &reporter.tx,
        local as u64,
    );
    // The hashing above reports past the reporter: send the next status
    // even when it equals the last one the reporter sent.
    reporter.last = None;
    let total_files = (plan.uploads.len()
        + plan.downloads.len()
        + plan.remote_deletes.len()
        + plan.local_deletes.len()
        + plan.remote_moves.len()
        + plan.local_moves.len()) as u64;
    if total_files == 0 {
        return Ok(out);
    }
    // A move without a stored copy uploads: count its bytes up front, and
    // take them off again when the server copies the file.
    let up_total = plan.uploads.iter().map(|u| u.local.size).sum::<u64>()
        + plan.remote_moves.iter().map(|m| m.upload.local.size).sum::<u64>();
    let down_total = plan.downloads.iter().map(|d| d.remote.size).sum::<u64>();
    let total_bytes = up_total + down_total;
    let transfer = Transfer {
        total_files,
        total_bytes,
        up_total,
        down_total,
        up_left: up_total,
        down_left: down_total,
        ..Transfer::default()
    };
    reporter.status(true, "syncing", local, total_files, total_bytes, transfer);

    ctx.files_done.store(0, Ordering::Relaxed);
    ctx.up_skipped.store(0, Ordering::Relaxed);
    let base = (ctx.store.bytes_sent(), ctx.store.bytes_received());
    let finished = AtomicBool::new(false);
    let tx = reporter.tx.clone();
    let result = thread::scope(|scope| {
        scope.spawn(|| progress_ticker(ctx, &tx, &finished, local as u64, transfer, base));
        let result = run_plan(ctx, state, &mut plan, reporter, &mut out);
        finished.store(true, Ordering::Release);
        result
    });
    result?;
    Ok(out)
}

/// Sends live progress (files and bytes left, speed) until `finished`.
fn progress_ticker(
    ctx: &Ctx,
    tx: &Sender<AppCommand>,
    finished: &AtomicBool,
    local: u64,
    transfer: Transfer,
    base: (u64, u64),
) {
    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    let mut up_samples: VecDeque<(Instant, u64)> = VecDeque::new();
    let mut down_samples: VecDeque<(Instant, u64)> = VecDeque::new();
    loop {
        let wait_until = Instant::now() + PROGRESS_TICK;
        while Instant::now() < wait_until {
            if finished.load(Ordering::Acquire) || ctx.stopping() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let now = Instant::now();
        let sent = ctx.store.bytes_sent().saturating_sub(base.0);
        let received = ctx.store.bytes_received().saturating_sub(base.1);
        let moved = sent + received;
        for (q, v) in [
            (&mut samples, moved),
            (&mut up_samples, sent),
            (&mut down_samples, received),
        ] {
            q.push_back((now, v));
            while q.len() > 2 && now.duration_since(q[0].0) > RATE_WINDOW {
                q.pop_front();
            }
        }
        let rate_bps = transfer_rate(&samples);
        let done = ctx.files_done.load(Ordering::Relaxed);
        let skipped = ctx.up_skipped.load(Ordering::Relaxed);
        let _ = tx.send(AppCommand::EngineStatus {
            connected: true,
            folder_state: "syncing".into(),
            local_files: local,
            need_files: transfer.total_files.saturating_sub(done),
            need_bytes: transfer.total_bytes.saturating_sub(moved + skipped),
            transfer: Transfer {
                rate_bps,
                up_left: transfer.up_total.saturating_sub(sent + skipped),
                down_left: transfer.down_total.saturating_sub(received),
                up_bps: transfer_rate(&up_samples),
                down_bps: transfer_rate(&down_samples),
                ..transfer
            },
        });
    }
}

/// Bytes per second between the oldest and newest sample.
fn transfer_rate(samples: &VecDeque<(Instant, u64)>) -> u64 {
    match (samples.front(), samples.back()) {
        (Some(first), Some(last)) if last.0 > first.0 => {
            let secs = last.0.duration_since(first.0).as_secs_f64();
            (last.1.saturating_sub(first.1) as f64 / secs) as u64
        }
        _ => 0,
    }
}

fn run_plan(
    ctx: &Ctx,
    state: &mut SyncState,
    plan: &mut Plan,
    reporter: &mut Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    // Moves first: a failed one falls back to the transfers below.
    move_local_all(ctx, state, plan, reporter, out);
    move_remote_all(ctx, state, plan, reporter, out)?;
    if ctx.stopping() {
        return Ok(());
    }
    upload_all(ctx, state, &plan.uploads, reporter, out)?;
    if ctx.stopping() {
        return Ok(());
    }
    download_all(ctx, state, &plan.downloads, reporter, out)?;

    for group in plan.remote_deletes.chunks(BATCH_FILES) {
        if ctx.stopping() {
            return Ok(());
        }
        delete_remote_batch(ctx, state, group, reporter, out)?;
        ctx.files_done
            .fetch_add(group.len() as u64, Ordering::Relaxed);
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
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
    }
    reporter.activity("Removed", &removed);
    Ok(())
}

/// Decide every path. Paths that only need a state update are applied here.
fn make_plan(
    ctx: &Ctx,
    state: &mut SyncState,
    files: &HashMap<String, PathBuf>,
    remote: &HashMap<String, RemoteFile>,
    out: &mut Outcome,
    tx: &Sender<AppCommand>,
    local_files: u64,
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
    // Files this device holds unchanged, by bucket ETag: a new bucket file
    // with the same bytes is placed from them instead of downloaded.
    let mut sources: HashMap<String, Vec<(String, Local, bool)>> = HashMap::new();
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
        let decision = decide(tip, local, remote.get(rel), keep_local);
        if let (Some(t), Some(l), Decision::Skip | Decision::DeleteLocal) = (tip, local, decision) {
            if is_md5_etag(&t.etag) && t.size > 0 {
                let rename = decision == Decision::DeleteLocal;
                sources
                    .entry(t.etag.to_ascii_lowercase())
                    .or_default()
                    .push((rel.clone(), l, rename));
            }
        }
        match decision {
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

    pair_local_moves(&mut plan, sources);
    // New local files that may be renamed bucket files: hash them too.
    let candidates = remote_move_candidates(ctx, state, &plan, remote);

    // Hash the files that exist on both sides, in parallel, and show how far
    // the check is: on a big folder this is the slow part of a round.
    let hash_jobs: Vec<(&String, u64)> = to_compare
        .iter()
        .map(|(rel, _)| (*rel, remote.get(*rel).map_or(0, |r| r.size)))
        .chain(candidates.iter().map(|(rel, size)| (rel, *size)))
        .collect();
    let check_files = hash_jobs.len() as u64;
    let check_bytes: u64 = hash_jobs.iter().map(|(_, size)| size).sum();
    let checked_files = AtomicU64::new(0);
    let checked_bytes = AtomicU64::new(0);
    let hashed = AtomicBool::new(false);
    let hashes = thread::scope(|scope| {
        if check_files > 0 {
            scope.spawn(|| loop {
                let _ = tx.send(AppCommand::EngineStatus {
                    connected: true,
                    folder_state: "scanning".into(),
                    local_files,
                    need_files: 0,
                    need_bytes: 0,
                    transfer: Transfer {
                        check_files,
                        check_bytes,
                        checked_files: checked_files.load(Ordering::Relaxed),
                        checked_bytes: checked_bytes.load(Ordering::Relaxed),
                        ..Transfer::default()
                    },
                });
                let wait_until = Instant::now() + PROGRESS_TICK;
                while Instant::now() < wait_until {
                    if hashed.load(Ordering::Acquire) || ctx.stopping() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            });
        }
        let hashes = pool::parallel_map(&hash_jobs, HASH_WORKERS, |(rel, size)| {
            if ctx.stopping() {
                return Err("stopped".to_string());
            }
            let result = hash_file(&files[*rel]).map(|(md5, _)| md5);
            checked_files.fetch_add(1, Ordering::Relaxed);
            checked_bytes.fetch_add(*size, Ordering::Relaxed);
            result
        });
        hashed.store(true, Ordering::Release);
        hashes
    });
    let mut hashes = hashes.into_iter();
    let compare_hashes: Vec<_> = hashes.by_ref().take(to_compare.len()).collect();
    pair_remote_moves(state, &mut plan, candidates.into_iter().zip(hashes));
    for ((rel, different), hash) in to_compare.into_iter().zip(compare_hashes) {
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

/// Turn downloads into local renames or copies when this device already
/// holds the same bytes (same ETag, same size) at another path. A folder
/// moved on another device then costs no download here.
fn pair_local_moves(plan: &mut Plan, mut sources: HashMap<String, Vec<(String, Local, bool)>>) {
    if sources.is_empty() {
        return;
    }
    let mut renamed = HashSet::new();
    let mut downloads = Vec::new();
    for d in std::mem::take(&mut plan.downloads) {
        // Only new paths: an existing local file is a real edit.
        let found = match d.local {
            None => sources
                .get_mut(&d.remote.etag.to_ascii_lowercase())
                .and_then(|list| {
                    // Prefer a file that left the bucket: rename it. Each
                    // renamed file is used once; a kept file can feed copies.
                    let fits = |(from, l, rename): &(String, Local, bool)| {
                        l.size == d.remote.size && !(*rename && renamed.contains(from))
                    };
                    let i = list
                        .iter()
                        .position(|s| s.2 && fits(s))
                        .or_else(|| list.iter().position(fits))?;
                    let (from, l, rename) = list[i].clone();
                    if rename {
                        renamed.insert(from.clone());
                    }
                    Some((from, l, rename))
                }),
            Some(_) => None,
        };
        match found {
            Some((from, from_local, rename)) => plan.local_moves.push(LocalMove {
                from,
                from_local,
                rename,
                download: d,
            }),
            None => downloads.push(d),
        }
    }
    plan.downloads = downloads;
    plan.local_deletes.retain(|rel| !renamed.contains(rel));
}

/// Uploads of new paths whose size matches a file this device deleted from
/// the bucket in the same round, or big enough to look up in the bucket
/// history. Only these are hashed for move detection.
fn remote_move_candidates(
    ctx: &Ctx,
    state: &SyncState,
    plan: &Plan,
    remote: &HashMap<String, RemoteFile>,
) -> Vec<(String, u64)> {
    let sizes: HashSet<u64> = plan
        .remote_deletes
        .iter()
        .filter_map(|rel| state.files.get(rel))
        .filter(|t| is_md5_etag(&t.etag) && t.size > 0)
        .map(|t| t.size)
        .collect();
    let failed = ctx.move_failed.lock().unwrap_or_else(|p| p.into_inner());
    plan.uploads
        .iter()
        .filter(|u| {
            (sizes.contains(&u.local.size) || u.local.size >= REUSE_MIN_BYTES)
                && !remote.contains_key(&u.rel)
                && !failed.contains(&u.rel)
        })
        .map(|u| (u.rel.clone(), u.local.size))
        .collect()
}

/// Pair hashed new files with deleted bucket files of the same MD5: each
/// pair becomes one server-side move instead of an upload plus a delete.
/// Other big new files ask the server for a stored copy before upload.
fn pair_remote_moves(
    state: &SyncState,
    plan: &mut Plan,
    hashed: impl Iterator<Item = ((String, u64), Result<String, String>)>,
) {
    let mut gone: HashMap<(String, u64), Vec<String>> = HashMap::new();
    for rel in &plan.remote_deletes {
        if let Some(t) = state.files.get(rel) {
            if is_md5_etag(&t.etag) {
                gone.entry((t.etag.to_ascii_lowercase(), t.size))
                    .or_default()
                    .push(rel.clone());
            }
        }
    }
    let mut pairs: HashMap<String, (Option<String>, String)> = HashMap::new();
    for ((rel, size), hash) in hashed {
        let Ok(md5) = hash else { continue };
        let key = (md5.to_ascii_lowercase(), size);
        match gone.get_mut(&key).and_then(Vec::pop) {
            Some(from) => {
                pairs.insert(rel, (Some(from), key.0));
            }
            None if size >= REUSE_MIN_BYTES => {
                pairs.insert(rel, (None, key.0));
            }
            None => {}
        }
    }
    if pairs.is_empty() {
        return;
    }
    let moved_from: HashSet<&String> = pairs.values().filter_map(|(from, _)| from.as_ref()).collect();
    plan.remote_deletes.retain(|rel| !moved_from.contains(rel));
    let mut uploads = Vec::new();
    for u in std::mem::take(&mut plan.uploads) {
        match pairs.remove(&u.rel) {
            Some((from, etag)) => plan.remote_moves.push(RemoteMove {
                from,
                etag,
                upload: u,
            }),
            None => uploads.push(u),
        }
    }
    plan.uploads = uploads;
}

/// Place files from bytes already on disk. A failure falls back to a
/// download (and a local delete for a rename).
fn move_local_all(
    ctx: &Ctx,
    state: &mut SyncState,
    plan: &mut Plan,
    reporter: &Reporter,
    out: &mut Outcome,
) {
    let mut moved = Vec::new();
    for m in std::mem::take(&mut plan.local_moves) {
        if ctx.stopping() {
            return;
        }
        let to = m.download.rel.clone();
        match move_local(ctx, &m) {
            Ok(mtime_ns) => {
                if m.rename {
                    state.files.remove(&m.from);
                }
                state.files.insert(
                    to.clone(),
                    Synced {
                        etag: m.download.remote.etag.clone(),
                        size: m.download.remote.size,
                        mtime_ns,
                    },
                );
                moved.push(to);
                out.changed = true;
            }
            Err(err) => {
                logs::append(&format!(
                    "sync: local move {} -> {to} failed: {err}",
                    m.from
                ));
                if m.rename {
                    plan.local_deletes.push(m.from);
                }
                plan.downloads.push(m.download);
            }
        }
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
    }
    if !moved.is_empty() {
        logs::append(&format!(
            "sync: placed {} file(s) from local copies",
            moved.len()
        ));
    }
    reporter.activity("Moved", &moved);
}

/// Rename (or copy) one file on disk. Ok carries the new file's mtime.
fn move_local(ctx: &Ctx, m: &LocalMove) -> Result<u64, String> {
    let from = safe_join(&ctx.root, &m.from)?;
    let to = safe_join(&ctx.root, &m.download.rel)?;
    let unchanged = |path: &Path| {
        file_fingerprint(path).ok() == Some((m.from_local.size, m.from_local.mtime_ns))
    };
    if !unchanged(&from) {
        return Err("source changed".into());
    }
    if fs::symlink_metadata(&to).is_ok() {
        return Err("target exists".into());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    if m.rename {
        fs::rename(&from, &to).map_err(|e| format!("rename: {e}"))?;
        return Ok(file_mtime_ns(&to));
    }
    // Copy through a temp file, then check the source did not change.
    let tmp = temp_path_for(&to);
    let copied = fs::copy(&from, &tmp).map_err(|e| format!("copy: {e}"));
    if copied.is_err() || !unchanged(&from) || fs::symlink_metadata(&to).is_ok() {
        let _ = fs::remove_file(&tmp);
        return Err(copied.err().unwrap_or_else(|| CHANGING.into()));
    }
    if let Err(err) = fs::rename(&tmp, &to) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("place: {err}"));
    }
    Ok(file_mtime_ns(&to))
}

/// Ask the server to rename objects inside the store. A rejected move
/// uploads the file and deletes the old key instead. A failed call skips
/// both for this round; the next round sees what really happened and does
/// not try that move again.
fn move_remote_all(
    ctx: &Ctx,
    state: &mut SyncState,
    plan: &mut Plan,
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    let moves = std::mem::take(&mut plan.remote_moves);
    let sizes: Vec<u64> = moves.iter().map(|m| m.upload.local.size).collect();
    let mut moved = Vec::new();
    let mut reused = 0usize;
    for range in chunks_by(&sizes, MOVE_BATCH_FILES, MOVE_BATCH_BYTES) {
        if ctx.stopping() {
            break;
        }
        let group = &moves[range];
        let requests: Vec<MoveRequest> = group
            .iter()
            .map(|m| MoveRequest {
                from: m.from.clone(),
                to: m.upload.rel.clone(),
                etag: m.etag.clone(),
            })
            .collect();
        for m in group {
            reporter.line(format!("Moving {}", m.upload.rel));
        }
        let results = match ctx.api.move_files(&requests) {
            Ok(results) => results,
            Err(ApiError::Auth(err)) => return Err(ApiError::Auth(err)),
            Err(err) => {
                logs::append(&format!("sync: move call failed: {err}"));
                let mut failed = ctx.move_failed.lock().unwrap_or_else(|p| p.into_inner());
                failed.extend(group.iter().map(|m| m.upload.rel.clone()));
                out.retry = true;
                ctx.files_done
                    .fetch_add(group.len() as u64, Ordering::Relaxed);
                ctx.up_skipped.fetch_add(
                    group.iter().map(|m| m.upload.local.size).sum(),
                    Ordering::Relaxed,
                );
                continue;
            }
        };
        for (m, result) in group.iter().zip(results) {
            let u = &m.upload;
            match result {
                Ok(etag) => {
                    ctx.files_done.fetch_add(1, Ordering::Relaxed);
                    ctx.up_skipped.fetch_add(u.local.size, Ordering::Relaxed);
                    match &m.from {
                        Some(from) => {
                            state.files.remove(from);
                        }
                        None => reused += 1,
                    }
                    // A file edited since the hash uploads next round.
                    if file_fingerprint(&u.abs).ok() == Some((u.local.size, u.local.mtime_ns)) {
                        state.files.insert(
                            u.rel.clone(),
                            Synced {
                                etag,
                                size: u.local.size,
                                mtime_ns: u.local.mtime_ns,
                            },
                        );
                    } else {
                        out.retry = true;
                    }
                    moved.push(u.rel.clone());
                    out.changed = true;
                }
                // No stored copy: the normal case for a new file.
                Err(err) if m.from.is_none() && err == "Not found." => {
                    ctx.move_failed
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(u.rel.clone());
                    plan.uploads.push(Upload {
                        rel: u.rel.clone(),
                        abs: u.abs.clone(),
                        local: u.local,
                    });
                }
                Err(err) => {
                    let from = m.from.as_deref().unwrap_or("bucket history");
                    logs::append(&format!("sync: move {from} -> {} failed: {err}", u.rel));
                    ctx.move_failed
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(u.rel.clone());
                    plan.uploads.push(Upload {
                        rel: u.rel.clone(),
                        abs: u.abs.clone(),
                        local: u.local,
                    });
                    plan.remote_deletes.extend(m.from.clone());
                }
            }
        }
        // A fallback counts as done when its upload ends.
    }
    if !moved.is_empty() {
        logs::append(&format!(
            "sync: moved {} file(s) in the bucket ({reused} from history)",
            moved.len()
        ));
    }
    reporter.activity("Moved", &moved);
    Ok(())
}

/// Split work into batches bounded by file count and bytes.
fn chunks(sizes: &[u64]) -> Vec<Range<usize>> {
    chunks_by(sizes, BATCH_FILES, BATCH_BYTES)
}

fn chunks_by(sizes: &[u64], max_files: usize, max_bytes: u64) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bytes = 0u64;
    for (i, size) in sizes.iter().enumerate() {
        let full = i - start >= max_files || (i > start && bytes + size > max_bytes);
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

/// Signs URLs one batch at a time, when the first worker reaches the batch.
/// URLs stay fresh on slow links, and no batch waits for another to finish.
struct UrlBook<'a> {
    ctx: &'a Ctx<'a>,
    method: Method,
    paths: Vec<String>,
    batches: Vec<Range<usize>>,
    signed: Vec<OnceLock<Result<Vec<String>, String>>>,
    /// First signing error; it ends the round after the workers stop.
    error: Mutex<Option<ApiError>>,
}

impl<'a> UrlBook<'a> {
    fn new(ctx: &'a Ctx<'a>, method: Method, paths: Vec<String>, sizes: &[u64]) -> Self {
        let batches = chunks(sizes);
        let signed = batches.iter().map(|_| OnceLock::new()).collect();
        Self {
            ctx,
            method,
            paths,
            batches,
            signed,
            error: Mutex::new(None),
        }
    }

    fn url(&self, index: usize) -> Result<String, String> {
        let batch = self.batches.partition_point(|range| range.end <= index);
        let range = self.batches[batch].clone();
        let urls = self.signed[batch].get_or_init(|| {
            if let Some(err) = self
                .error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                return Err(err.to_string());
            }
            self.ctx
                .api
                .sign(self.method, &self.paths[range.clone()])
                .map_err(|err| {
                    let text = err.to_string();
                    let mut first = self.error.lock().unwrap_or_else(|p| p.into_inner());
                    first.get_or_insert(err);
                    text
                })
        });
        match urls {
            Ok(urls) => Ok(urls[index - range.start].clone()),
            Err(err) => Err(format!("sign: {err}")),
        }
    }

    fn into_error(self) -> Option<ApiError> {
        self.error.into_inner().unwrap_or_else(|p| p.into_inner())
    }
}

/// The file changed on disk during the transfer. Not a failure: the next
/// round moves the new bytes.
const CHANGING: &str = "changed while syncing; will retry";

fn upload_all(
    ctx: &Ctx,
    state: &mut SyncState,
    uploads: &[Upload],
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    if uploads.is_empty() {
        return Ok(());
    }
    let sizes: Vec<u64> = uploads.iter().map(|u| u.local.size).collect();
    let book = UrlBook::new(
        ctx,
        Method::Put,
        uploads.iter().map(|u| u.rel.clone()).collect(),
        &sizes,
    );
    let jobs: Vec<(usize, &Upload)> = uploads.iter().enumerate().collect();
    let results = pool::split_map(
        &jobs,
        |(_, u)| u.local.size >= BIG_FILE_BYTES,
        SMALL_WORKERS,
        BIG_WORKERS,
        |&(i, u)| {
            if ctx.stopping() {
                return Err("stopped".to_string());
            }
            let url = book.url(i)?;
            reporter.line(format!("Uploading {}", u.rel));
            let result = with_retries(ctx, || ctx.store.put_file(&url, &u.abs)).and_then(|etag| {
                // Stored bytes must be the bytes that were planned.
                match file_fingerprint(&u.abs) {
                    Ok((size, mtime_ns))
                        if (size, mtime_ns) == (u.local.size, u.local.mtime_ns) =>
                    {
                        Ok(etag)
                    }
                    _ => Err(CHANGING.to_string()),
                }
            });
            ctx.files_done.fetch_add(1, Ordering::Relaxed);
            match &result {
                Ok(_) => reporter.line(format!("Uploaded {}", u.rel)),
                Err(err) if err == CHANGING => {
                    reporter.line(format!("Waiting to upload {}", u.rel))
                }
                Err(err) => reporter.line(format!("Could not sync {}: {err}", u.rel)),
            }
            result
        },
    );
    let mut done = 0usize;
    for ((_, u), result) in jobs.iter().zip(results) {
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
                done += 1;
            }
            Err(err) => {
                logs::append(&format!("sync: upload {} failed: {err}", u.rel));
                out.retry = true;
            }
        }
    }
    out.changed |= done > 0;
    logs::append(&format!("sync: uploaded {done} file(s)"));
    book.into_error().map_or(Ok(()), Err)
}

fn download_all(
    ctx: &Ctx,
    state: &mut SyncState,
    downloads: &[Download],
    reporter: &Reporter,
    out: &mut Outcome,
) -> Result<(), ApiError> {
    if downloads.is_empty() {
        return Ok(());
    }
    let sizes: Vec<u64> = downloads.iter().map(|d| d.remote.size).collect();
    let book = UrlBook::new(
        ctx,
        Method::Get,
        downloads.iter().map(|d| d.rel.clone()).collect(),
        &sizes,
    );
    let jobs: Vec<(usize, &Download)> = downloads.iter().enumerate().collect();
    // Each worker downloads, checks and places one file; Ok carries its mtime.
    let results = pool::split_map(
        &jobs,
        |(_, d)| d.remote.size >= BIG_FILE_BYTES,
        SMALL_WORKERS,
        BIG_WORKERS,
        |&(i, d)| {
            if ctx.stopping() {
                return Err("stopped".to_string());
            }
            let result = download_one(ctx, &book, i, d, reporter);
            ctx.files_done.fetch_add(1, Ordering::Relaxed);
            match &result {
                Ok(_) => reporter.line(format!("Downloaded {}", d.rel)),
                Err(err) if err == CHANGING => {
                    reporter.line(format!("Waiting to download {}", d.rel))
                }
                Err(err) => reporter.line(format!("Could not sync {}: {err}", d.rel)),
            }
            result
        },
    );
    let mut done = 0usize;
    for ((_, d), result) in jobs.iter().zip(results) {
        match result {
            Ok(mtime_ns) => {
                state.files.insert(
                    d.rel.clone(),
                    Synced {
                        etag: d.remote.etag.clone(),
                        size: d.remote.size,
                        mtime_ns,
                    },
                );
                done += 1;
            }
            Err(err) => {
                logs::append(&format!("sync: download {} failed: {err}", d.rel));
                out.retry = true;
            }
        }
    }
    out.changed |= done > 0;
    logs::append(&format!("sync: downloaded {done} file(s)"));
    book.into_error().map_or(Ok(()), Err)
}

/// Download one file to a temp file next to it, then rename it into place.
fn download_one(
    ctx: &Ctx,
    book: &UrlBook,
    index: usize,
    d: &Download,
    reporter: &Reporter,
) -> Result<u64, String> {
    let abs = safe_join(&ctx.root, &d.rel)?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // Temp file next to the target, so the final rename is atomic.
    let tmp = temp_path_for(&abs);
    let url = book.url(index)?;
    reporter.line(format!("Downloading {}", d.rel));
    // Only a plain MD5 ETag proves the bytes; multipart ETags do not.
    let md5 = if is_md5_etag(&d.remote.etag) {
        d.remote.etag.as_str()
    } else {
        ""
    };
    with_retries(ctx, || {
        ctx.store.get_file_fast(&url, &tmp, d.remote.size, md5)
    })?;
    // The user edited the file while it downloaded: keep their edit.
    let now = file_fingerprint(&abs)
        .ok()
        .map(|(size, mtime_ns)| Local { size, mtime_ns });
    if now != d.local {
        let _ = fs::remove_file(&tmp);
        return Err(CHANGING.into());
    }
    if let Err(err) = fs::rename(&tmp, &abs) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("place: {err}"));
    }
    Ok(file_mtime_ns(&abs))
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

/// Delete our temp files under `root`, left when the app was killed mid-download.
fn remove_stale_temps(root: &Path) {
    let mut removed = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() && !ft.is_symlink() {
                stack.push(path);
            } else if ft.is_file()
                && entry.file_name().to_string_lossy().ends_with(TEMP_SUFFIX)
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    if removed > 0 {
        logs::append(&format!("sync: removed {removed} leftover temp file(s)"));
    }
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

    fn md5(n: u8) -> String {
        format!("{:032x}", n)
    }

    fn synced(etag: &str, size: u64) -> Synced {
        Synced {
            etag: etag.into(),
            size,
            mtime_ns: 1,
        }
    }

    fn upload(rel: &str, size: u64) -> Upload {
        Upload {
            rel: rel.into(),
            abs: PathBuf::from(rel),
            local: Local { size, mtime_ns: 2 },
        }
    }

    fn download(rel: &str, etag: &str, size: u64) -> Download {
        Download {
            rel: rel.into(),
            remote: RemoteFile {
                path: rel.into(),
                size,
                etag: etag.into(),
                modified: "2026-01-02T03:04:05.000Z".into(),
            },
            local: None,
        }
    }

    #[test]
    fn renamed_files_become_remote_moves() {
        let mut state = SyncState::default();
        state.files.insert("old/a.bin".into(), synced(&md5(1), 10));
        state.files.insert("old/b.bin".into(), synced(&md5(2), 10));
        let mut plan = Plan {
            uploads: vec![upload("new/a.bin", 10), upload("new/c.bin", 10)],
            remote_deletes: vec!["old/a.bin".into(), "old/b.bin".into()],
            ..Plan::default()
        };
        let hashed = vec![
            (("new/a.bin".to_string(), 10), Ok(md5(1).to_uppercase())),
            // Same size, other bytes: stays an upload.
            (("new/c.bin".to_string(), 10), Ok(md5(3))),
        ];
        pair_remote_moves(&state, &mut plan, hashed.into_iter());
        assert_eq!(plan.remote_moves.len(), 1);
        assert_eq!(plan.remote_moves[0].from.as_deref(), Some("old/a.bin"));
        assert_eq!(plan.remote_moves[0].upload.rel, "new/a.bin");
        assert_eq!(plan.uploads.len(), 1);
        assert_eq!(plan.uploads[0].rel, "new/c.bin");
        assert_eq!(plan.remote_deletes, vec!["old/b.bin".to_string()]);
    }

    #[test]
    fn big_new_files_ask_the_bucket_history_first() {
        let state = SyncState::default();
        let big = REUSE_MIN_BYTES;
        let mut plan = Plan {
            uploads: vec![upload("new/big.bin", big), upload("new/small.bin", 10)],
            ..Plan::default()
        };
        let hashed = vec![
            (("new/big.bin".to_string(), big), Ok(md5(4))),
            (("new/small.bin".to_string(), 10), Ok(md5(5))),
        ];
        pair_remote_moves(&state, &mut plan, hashed.into_iter());
        assert_eq!(plan.remote_moves.len(), 1);
        assert_eq!(plan.remote_moves[0].from, None);
        assert_eq!(plan.remote_moves[0].etag, md5(4));
        assert_eq!(plan.uploads.len(), 1);
        assert_eq!(plan.uploads[0].rel, "new/small.bin");
    }

    #[test]
    fn new_bucket_files_are_placed_from_local_bytes() {
        let held = Local {
            size: 10,
            mtime_ns: 1,
        };
        let mut sources: HashMap<String, Vec<(String, Local, bool)>> = HashMap::new();
        // a.bin left the bucket: rename. k.bin stays: copy.
        sources.insert(md5(1), vec![("old/a.bin".into(), held, true)]);
        sources.insert(md5(2), vec![("keep/k.bin".into(), held, false)]);
        let mut plan = Plan {
            downloads: vec![
                download("new/a.bin", &md5(1), 10),
                // A second new file with the same bytes: the rename source
                // is used once, so it downloads.
                download("new/a2.bin", &md5(1), 10),
                download("new/k.bin", &md5(2), 10),
                download("new/other.bin", &md5(3), 10),
            ],
            local_deletes: vec!["old/a.bin".into()],
            ..Plan::default()
        };
        pair_local_moves(&mut plan, sources);
        let moves: Vec<_> = plan
            .local_moves
            .iter()
            .map(|m| (m.from.as_str(), m.download.rel.as_str(), m.rename))
            .collect();
        assert_eq!(
            moves,
            vec![
                ("old/a.bin", "new/a.bin", true),
                ("keep/k.bin", "new/k.bin", false)
            ]
        );
        let downloads: Vec<_> = plan.downloads.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(downloads, vec!["new/a2.bin", "new/other.bin"]);
        assert!(plan.local_deletes.is_empty());
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
                files_done: AtomicU64::new(0),
                up_skipped: AtomicU64::new(0),
                move_failed: Mutex::new(HashSet::new()),
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

        // 6. A moves a folder: the bucket copies it inside the store and B
        // renames its own files. No file bytes go up or down.
        fs::create_dir_all(a.root.join("moved")).unwrap();
        fs::rename(a.root.join("docs/deep"), a.root.join("moved/deep")).unwrap();
        for i in 10..20 {
            fs::rename(
                a.root.join(format!("docs/n{i:03}.txt")),
                a.root.join(format!("moved/n{i:03}.txt")),
            )
            .unwrap();
        }
        let (sent, received) = (a.store.bytes_sent(), b.store.bytes_received());
        sync(&a, &mut sa);
        sync(&b, &mut sb);
        same_tree(&a.root, &b.root);
        assert!(b.root.join("moved/deep/big.bin").exists());
        assert!(!b.root.join("docs/deep/big.bin").exists());
        assert_eq!(a.store.bytes_sent(), sent, "a move uploaded bytes");
        assert_eq!(
            b.store.bytes_received(),
            received,
            "a move downloaded bytes"
        );
        let listed: HashSet<String> = a
            .api
            .list_files()
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert!(listed.contains("moved/deep/big.bin") && !listed.contains("docs/deep/big.bin"));

        let _ = fs::remove_dir_all(&a.root);
        let _ = fs::remove_dir_all(&b.root);
    }
}
