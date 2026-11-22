//! Shared application state for the native Windows and macOS shells.
//!
//! The in-process file sync engine owns transfer scheduling. Native controls
//! send `AppCommand`s and render immutable `AppSnapshot`s; they never model a
//! transport queue or attempt individual-file retries.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;

pub const MAX_RECENT_ACTIVITY: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    ReconnectRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingState {
    Idle,
    Starting,
    AwaitingApproval {
        code: String,
        approve_url: String,
        api_base: String,
    },
    Applying,
    Failed {
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkState {
    Idle,
    Scanning,
    Syncing,
    PausedForReconnect,
}

/// Size of the current sync round and its live speed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transfer {
    pub total_files: u64,
    pub total_bytes: u64,
    /// Bytes per second, averaged over the last few seconds.
    pub rate_bps: u64,
    /// Upload and download bytes in this round.
    pub up_total: u64,
    pub down_total: u64,
    /// Upload and download bytes still to move.
    pub up_left: u64,
    pub down_left: u64,
    /// Upload and download speed, bytes per second.
    pub up_bps: u64,
    pub down_bps: u64,
    /// Files present on both sides, hashed before the transfer starts.
    pub check_files: u64,
    pub check_bytes: u64,
    pub checked_files: u64,
    pub checked_bytes: u64,
}

/// What one activity line says. The engine writes the lines; both UIs read
/// them with this, so the words stay the same on Windows and macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity<'a> {
    /// A file moving, or moved, in one direction.
    File {
        dir: Direction,
        done: bool,
        path: &'a str,
    },
    /// The file changed during the transfer; the next round tries again.
    Waiting { dir: Direction, path: &'a str },
    /// A file that could not sync, and why.
    Failed { path: &'a str, reason: &'a str },
    Info(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    /// Deleted here, so deleted on the server.
    DeletedRemote,
    /// Deleted on another device, so deleted here.
    DeletedLocal,
    /// Renamed or moved without moving the bytes again.
    Moved,
}

impl Direction {
    /// Arrow for upload, download and delete rows.
    pub fn icon(self) -> &'static str {
        match self {
            Direction::Up => "\u{2191}",
            Direction::Down => "\u{2193}",
            Direction::DeletedRemote | Direction::DeletedLocal => "\u{2715}",
            Direction::Moved => "\u{2192}",
        }
    }

    /// "Uploading recipe.md", "Downloaded recipe.md", "Deleted recipe.md (here)".
    pub fn label(self, done: bool, name: &str) -> String {
        match (self, done) {
            (Direction::Up, false) => format!("Uploading {name}"),
            (Direction::Up, true) => format!("Uploaded {name}"),
            (Direction::Down, false) => format!("Downloading {name}"),
            (Direction::Down, true) => format!("Downloaded {name}"),
            (Direction::DeletedRemote, _) => format!("Deleted {name} (from server)"),
            (Direction::DeletedLocal, _) => format!("Deleted {name} (here)"),
            (Direction::Moved, false) => format!("Moving {name}"),
            (Direction::Moved, true) => format!("Moved {name}"),
        }
    }
}

pub fn parse_activity(line: &str) -> Option<Activity<'_>> {
    let file = |dir, done, path| Some(Activity::File { dir, done, path });
    for (prefix, dir, done) in [
        ("Uploading ", Direction::Up, false),
        ("Uploaded ", Direction::Up, true),
        ("Downloading ", Direction::Down, false),
        ("Downloaded ", Direction::Down, true),
        ("Deleted ", Direction::DeletedRemote, true),
        ("Removed ", Direction::DeletedLocal, true),
        ("Moving ", Direction::Moved, false),
        ("Moved ", Direction::Moved, true),
    ] {
        if let Some(path) = line.strip_prefix(prefix) {
            return file(dir, done, path);
        }
    }
    for (prefix, dir) in [
        ("Waiting to upload ", Direction::Up),
        ("Waiting to download ", Direction::Down),
    ] {
        if let Some(path) = line.strip_prefix(prefix) {
            return Some(Activity::Waiting { dir, path });
        }
    }
    if let Some(detail) = line.strip_prefix("Could not sync ") {
        let (path, reason) = detail.split_once(": ").unwrap_or((detail, ""));
        return Some(Activity::Failed { path, reason });
    }
    line.strip_prefix("! ").map(Activity::Info)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSnapshot {
    pub connection: ConnectionState,
    pub pairing: PairingState,
    pub work: WorkState,
    pub watch_folder: Option<PathBuf>,
    pub pair_api_base: String,
    pub folder_label: String,
    pub folder_state: String,
    pub start_at_login: bool,
    pub auto_update: bool,
    pub hub_connected: bool,
    pub local_files: u64,
    pub global_files: u64,
    pub need_files: u64,
    pub need_bytes: u64,
    pub transfer: Transfer,
    pub activity: VecDeque<String>,
    pub last_event_id: u64,
}

impl AppSnapshot {
    /// Percent done in this sync round: by bytes, or by files when no bytes move.
    pub fn progress_pct(&self) -> u64 {
        let t = self.transfer;
        (t.total_bytes.saturating_sub(self.need_bytes) * 100)
            .checked_div(t.total_bytes)
            .or_else(|| {
                (t.total_files.saturating_sub(self.need_files) * 100).checked_div(t.total_files)
            })
            .unwrap_or(0)
    }

    /// "Upload 2.0 GB · 5.1 MB/s   Download 10.3 GB · 20.0 MB/s": bytes left
    /// and speed per direction. A direction with nothing to move is left out.
    pub fn direction_detail(&self) -> String {
        let t = self.transfer;
        let mut parts = Vec::new();
        for (name, total, left, bps) in [
            ("Upload", t.up_total, t.up_left, t.up_bps),
            ("Download", t.down_total, t.down_left, t.down_bps),
        ] {
            if total == 0 {
                continue;
            }
            let mut part = format!("{name} {}", format_bytes(left));
            if bps > 0 {
                part.push_str(&format!(" · {}/s", format_bytes(bps)));
            }
            parts.push(part);
        }
        parts.join("   ")
    }

    /// "Comparing 120 of 3400 files · 4.1 GB of 20.0 GB" while hashing.
    pub fn checking_detail(&self) -> String {
        let t = self.transfer;
        if t.check_files == 0 {
            return String::new();
        }
        format!(
            "Comparing {} of {} files · {} of {}",
            t.checked_files,
            t.check_files,
            format_bytes(t.checked_bytes),
            format_bytes(t.check_bytes)
        )
    }

    /// Percent of the hashing done, by bytes.
    pub fn checking_pct(&self) -> u64 {
        let t = self.transfer;
        (t.checked_bytes * 100)
            .checked_div(t.check_bytes)
            .or_else(|| (t.checked_files * 100).checked_div(t.check_files))
            .unwrap_or(0)
    }

    /// Seconds left at the current speed.
    pub fn eta_secs(&self) -> Option<u64> {
        let rate = self.transfer.rate_bps;
        (rate > 0 && self.need_bytes > 0).then(|| self.need_bytes / rate)
    }
}

/// "512 B", "3.4 KB", "12.0 MB", "2.1 GB".
pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// "45 s left", "12 min left", "2 h 5 min left".
pub fn format_eta(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s left")
    } else if secs < 3600 {
        format!("{} min left", secs.div_ceil(60))
    } else {
        format!("{} h {} min left", secs / 3600, (secs % 3600) / 60)
    }
}

impl Default for AppSnapshot {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            pairing: PairingState::Idle,
            work: WorkState::Idle,
            watch_folder: None,
            pair_api_base: "https://backup.rui.cam".to_string(),
            folder_label: String::new(),
            folder_state: String::new(),
            start_at_login: true,
            auto_update: true,
            hub_connected: false,
            local_files: 0,
            global_files: 0,
            need_files: 0,
            need_bytes: 0,
            transfer: Transfer::default(),
            activity: VecDeque::new(),
            last_event_id: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppCommand {
    SetWatchFolder(PathBuf),
    SetPairApiBase(String),
    Connect,
    PairStarted {
        code: String,
        approve_url: String,
        api_base: String,
    },
    PairApproved,
    PairFailed {
        message: String,
        retryable: bool,
    },
    CancelPairing,
    EngineStarting,
    /// Update work phase after the in-process file sync engine starts or settles.
    #[cfg_attr(windows, allow(dead_code))] // macOS shell only
    Work(WorkState),
    EngineStatus {
        connected: bool,
        folder_state: String,
        local_files: u64,
        need_files: u64,
        need_bytes: u64,
        transfer: Transfer,
    },
    Activity(String),
    EngineFailed(String),
    #[cfg_attr(not(windows), allow(dead_code))] // Windows shell only
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    SnapshotChanged(Box<AppSnapshot>),
    ConnectRequested { api_base: String },
    PairCancellationRequested,
    Shutdown,
}

#[derive(Clone)]
pub struct AppHandle {
    pub commands: Sender<AppCommand>,
    pub snapshot: Arc<RwLock<AppSnapshot>>,
}

impl AppHandle {
    pub fn send(&self, command: AppCommand) -> Result<(), mpsc::SendError<AppCommand>> {
        self.commands.send(command)
    }

    pub fn snapshot(&self) -> AppSnapshot {
        self.snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub struct AppController;

impl AppController {
    pub fn start(initial: AppSnapshot) -> (AppHandle, Receiver<AppEvent>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let shared = Arc::new(RwLock::new(initial.clone()));
        let handle = AppHandle {
            commands: command_tx,
            snapshot: shared.clone(),
        };
        thread::Builder::new()
            .name("app-controller".to_string())
            .spawn(move || run_controller(initial, shared, command_rx, event_tx))
            .expect("spawn application controller");
        (handle, event_rx)
    }
}

fn run_controller(
    mut state: AppSnapshot,
    shared: Arc<RwLock<AppSnapshot>>,
    commands: Receiver<AppCommand>,
    events: Sender<AppEvent>,
) {
    while let Ok(command) = commands.recv() {
        let shutdown = matches!(command, AppCommand::Shutdown);
        reduce(&mut state, command, &events);
        *shared
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state.clone();
        let _ = events.send(AppEvent::SnapshotChanged(Box::new(state.clone())));
        if shutdown {
            let _ = events.send(AppEvent::Shutdown);
            break;
        }
    }
}

fn reduce(state: &mut AppSnapshot, command: AppCommand, events: &Sender<AppEvent>) {
    match command {
        AppCommand::SetWatchFolder(path) => state.watch_folder = Some(path),
        AppCommand::SetPairApiBase(api_base) => state.pair_api_base = api_base,
        AppCommand::Connect => {
            state.connection = ConnectionState::Connecting;
            state.pairing = PairingState::Starting;
            let _ = events.send(AppEvent::ConnectRequested {
                api_base: state.pair_api_base.clone(),
            });
        }
        AppCommand::PairStarted {
            code,
            approve_url,
            api_base,
        } => {
            state.pairing = PairingState::AwaitingApproval {
                code,
                approve_url,
                api_base,
            };
        }
        AppCommand::PairApproved => state.pairing = PairingState::Applying,
        AppCommand::PairFailed { message, retryable } => {
            if !matches!(state.connection, ConnectionState::Connected) {
                state.connection = ConnectionState::Disconnected;
            }
            state.pairing = PairingState::Failed { message, retryable };
        }
        AppCommand::CancelPairing => {
            state.pairing = PairingState::Idle;
            if !state.hub_connected {
                state.connection = ConnectionState::Disconnected;
            }
            let _ = events.send(AppEvent::PairCancellationRequested);
        }
        AppCommand::EngineStarting => {
            state.connection = ConnectionState::Connecting;
            state.work = WorkState::Scanning;
        }
        AppCommand::Work(work) => {
            state.work = work;
            if state.work == WorkState::Idle {
                state.connection = ConnectionState::Connected;
                state.hub_connected = true;
                state.pairing = PairingState::Idle;
            }
        }
        AppCommand::EngineStatus {
            connected,
            folder_state,
            local_files,
            need_files,
            need_bytes,
            transfer,
        } => {
            state.hub_connected = connected;
            state.connection = if connected {
                ConnectionState::Connected
            } else {
                ConnectionState::Disconnected
            };
            state.folder_state = folder_state;
            state.local_files = local_files;
            state.global_files = local_files;
            state.need_files = need_files;
            state.need_bytes = need_bytes;
            state.transfer = transfer;
            state.pairing = PairingState::Idle;
            state.work = match state.folder_state.as_str() {
                "scanning" => WorkState::Scanning,
                "syncing" => WorkState::Syncing,
                _ if state.need_files > 0 || state.need_bytes > 0 => WorkState::Syncing,
                _ => WorkState::Idle,
            };
        }
        AppCommand::Activity(line) => {
            if state.activity.back() != Some(&line) {
                state.activity.push_back(line);
                state.last_event_id += 1;
                while state.activity.len() > MAX_RECENT_ACTIVITY {
                    state.activity.pop_front();
                }
            }
        }
        AppCommand::EngineFailed(reason) => {
            state.hub_connected = false;
            state.connection = ConnectionState::ReconnectRequired { reason };
            state.work = WorkState::PausedForReconnect;
        }
        AppCommand::Shutdown => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_maps_engine_state_and_counts() {
        let mut state = AppSnapshot::default();
        reduce(
            &mut state,
            AppCommand::EngineStatus {
                connected: true,
                folder_state: "syncing".into(),
                local_files: 5,
                need_files: 3,
                need_bytes: 4096,
                transfer: Transfer {
                    total_files: 10,
                    total_bytes: 8192,
                    rate_bps: 1000,
                    ..Transfer::default()
                },
            },
            &mpsc::channel().0,
        );
        assert_eq!(state.connection, ConnectionState::Connected);
        assert_eq!(state.work, WorkState::Syncing);
        assert_eq!(state.need_files, 3);
        assert_eq!(state.need_bytes, 4096);
    }

    #[test]
    fn progress_and_eta_use_bytes() {
        let state = AppSnapshot {
            need_files: 5,
            need_bytes: 2_000,
            transfer: Transfer {
                total_files: 10,
                total_bytes: 8_000,
                rate_bps: 100,
                ..Transfer::default()
            },
            ..AppSnapshot::default()
        };
        assert_eq!(state.progress_pct(), 75);
        assert_eq!(state.eta_secs(), Some(20));
        assert_eq!(format_eta(20), "20 s left");
        assert_eq!(format_eta(600), "10 min left");
        assert_eq!(format_eta(7500), "2 h 5 min left");
    }

    #[test]
    fn activity_lines_parse() {
        assert_eq!(
            parse_activity("Waiting to upload models/a.glb"),
            Some(Activity::Waiting { dir: Direction::Up, path: "models/a.glb" })
        );
        assert_eq!(
            parse_activity("Could not sync models/a.glb: stat failed"),
            Some(Activity::Failed { path: "models/a.glb", reason: "stat failed" })
        );
    }

    #[test]
    fn activity_lines_are_retained() {
        let mut state = AppSnapshot::default();
        reduce(
            &mut state,
            AppCommand::Activity("Uploaded Invoices/one.pdf".into()),
            &mpsc::channel().0,
        );
        assert_eq!(state.activity.back().unwrap(), "Uploaded Invoices/one.pdf");
        assert_eq!(state.last_event_id, 1);
    }
}
