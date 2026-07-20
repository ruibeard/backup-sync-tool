//! Shared application state for the native Windows and macOS shells.
//!
//! The in-process chunk sync engine owns transfer scheduling. Native controls
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
    pub activity: VecDeque<String>,
    pub last_event_id: u64,
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
            activity: VecDeque::new(),
            last_event_id: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppCommand {
    SetWatchFolder(PathBuf),
    SetPairApiBase(String),
    SetPreferences {
        start_at_login: bool,
        auto_update: bool,
    },
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
    /// Update work phase after the in-process chunk engine starts or settles.
    Work(WorkState),
    EngineStatus {
        connected: bool,
        folder_state: String,
        local_files: u64,
        need_files: u64,
        need_bytes: u64,
    },
    Activity(String),
    EngineFailed(String),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    SnapshotChanged(AppSnapshot),
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
        let _ = events.send(AppEvent::SnapshotChanged(state.clone()));
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
        AppCommand::SetPreferences {
            start_at_login,
            auto_update,
        } => {
            state.start_at_login = start_at_login;
            state.auto_update = auto_update;
        }
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
            },
            &mpsc::channel().0,
        );
        assert_eq!(state.connection, ConnectionState::Connected);
        assert_eq!(state.work, WorkState::Syncing);
        assert_eq!(state.need_files, 3);
        assert_eq!(state.need_bytes, 4096);
    }

    #[test]
    fn activity_lines_are_retained() {
        let mut state = AppSnapshot::default();
        reduce(
            &mut state,
            AppCommand::Activity("Synced Invoices/one.pdf".into()),
            &mpsc::channel().0,
        );
        assert_eq!(state.activity.back().unwrap(), "Synced Invoices/one.pdf");
    }
}
