//! Shared application state owner used by the native Windows and macOS shells.
//!
//! Native controls send `AppCommand`s and render immutable `AppSnapshot`s. The
//! controller thread is the only snapshot writer, which keeps pairing and
//! transfer state deterministic without introducing an async runtime.

use crate::sync::{TransferEvent, TransferKind, TransferState};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;

pub const MAX_CURRENT_RUN_ACTIVITY: usize = 200;

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
    Uploading,
    Restoring,
    PausedForReconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityStatus {
    Started,
    Progress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRow {
    pub id: u64,
    pub kind: TransferKind,
    pub relative_path: String,
    pub status: ActivityStatus,
    pub transferred: u64,
    pub total: u64,
    pub error: Option<String>,
}

impl ActivityRow {
    pub fn percent(&self) -> u8 {
        if self.total == 0 {
            return if self.status == ActivityStatus::Completed {
                100
            } else {
                0
            };
        }
        self.transferred
            .min(self.total)
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(0) as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSnapshot {
    pub connection: ConnectionState,
    pub pairing: PairingState,
    pub work: WorkState,
    pub watch_folder: Option<PathBuf>,
    pub pair_api_base: String,
    pub start_at_login: bool,
    pub auto_update: bool,
    pub activity: VecDeque<ActivityRow>,
    pub completed: usize,
    pub failed: usize,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub batch_start_id: u64,
}

impl Default for AppSnapshot {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            pairing: PairingState::Idle,
            work: WorkState::Idle,
            watch_folder: None,
            pair_api_base: "https://backup.rui.cam".to_string(),
            start_at_login: true,
            auto_update: true,
            activity: VecDeque::new(),
            completed: 0,
            failed: 0,
            transferred_bytes: 0,
            total_bytes: 0,
            batch_start_id: 0,
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
    ScanStarted,
    ScanFinished,
    Transfer(TransferEvent),
    RetryFailed,
    RestoreRequested(PathBuf),
    AuthFailed(String),
    ConnectionValidated,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    SnapshotChanged(AppSnapshot),
    ConnectRequested { api_base: String },
    PairCancellationRequested,
    RetryFailedRequested,
    RestoreRequested(PathBuf),
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

    pub fn transfer_event_callback(&self) -> crate::sync::TransferEventFn {
        let commands = self.commands.clone();
        Arc::new(move |event| {
            let _ = commands.send(AppCommand::Transfer(event));
        })
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
    let mut next_activity_id = state
        .activity
        .iter()
        .map(|row| row.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while let Ok(command) = commands.recv() {
        let shutdown = matches!(command, AppCommand::Shutdown);
        reduce(&mut state, command, &events, &mut next_activity_id);
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

fn reduce(
    state: &mut AppSnapshot,
    command: AppCommand,
    events: &Sender<AppEvent>,
    next_activity_id: &mut u64,
) {
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
            // A reconnect is transactional: keep representing the active
            // connection until replacement credentials are fully validated.
            if !matches!(state.connection, ConnectionState::Connected) {
                state.connection = ConnectionState::Connecting;
            }
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
            if !matches!(
                state.connection,
                ConnectionState::Connected | ConnectionState::ReconnectRequired { .. }
            ) {
                state.connection = ConnectionState::Disconnected;
            }
            state.pairing = PairingState::Failed { message, retryable };
        }
        AppCommand::CancelPairing => {
            state.pairing = PairingState::Idle;
            if !matches!(state.connection, ConnectionState::Connected) {
                state.connection = ConnectionState::Disconnected;
            }
            let _ = events.send(AppEvent::PairCancellationRequested);
        }
        AppCommand::ScanStarted => {
            state.work = WorkState::Scanning;
            state.batch_start_id = *next_activity_id;
            state.transferred_bytes = 0;
            state.total_bytes = 0;
        }
        AppCommand::ScanFinished => {
            if !matches!(state.work, WorkState::PausedForReconnect) {
                state.work = derive_work_state(&state.activity);
            }
        }
        AppCommand::Transfer(event) => {
            apply_transfer_event(state, event, next_activity_id);
        }
        AppCommand::RetryFailed => {
            let _ = events.send(AppEvent::RetryFailedRequested);
        }
        AppCommand::RestoreRequested(path) => {
            state.work = WorkState::Restoring;
            let _ = events.send(AppEvent::RestoreRequested(path));
        }
        AppCommand::AuthFailed(reason) => {
            state.connection = ConnectionState::ReconnectRequired { reason };
            state.work = WorkState::PausedForReconnect;
        }
        AppCommand::ConnectionValidated => {
            state.connection = ConnectionState::Connected;
            state.pairing = PairingState::Idle;
            if matches!(state.work, WorkState::PausedForReconnect) {
                state.work = WorkState::Idle;
            }
        }
        AppCommand::Shutdown => {}
    }
}

fn apply_transfer_event(
    snapshot: &mut AppSnapshot,
    event: TransferEvent,
    next_activity_id: &mut u64,
) {
    if event.auth_failed {
        snapshot.connection = ConnectionState::ReconnectRequired {
            reason: event
                .error
                .clone()
                .unwrap_or_else(|| "Storage credentials were rejected.".to_string()),
        };
        snapshot.work = WorkState::PausedForReconnect;
    }

    let status = match event.state {
        TransferState::Started => ActivityStatus::Started,
        TransferState::Progress => ActivityStatus::Progress,
        TransferState::Completed => ActivityStatus::Completed,
        TransferState::Failed => ActivityStatus::Failed,
        TransferState::Cancelled => ActivityStatus::Cancelled,
    };
    let active = snapshot.activity.iter_mut().rev().find(|row| {
        row.kind == event.kind
            && row.relative_path == event.relative_path
            && !matches!(
                row.status,
                ActivityStatus::Completed | ActivityStatus::Failed | ActivityStatus::Cancelled
            )
    });
    if let Some(row) = active {
        // Transport callbacks must be monotonic even when a resumed multipart
        // upload begins above zero or a stale progress message arrives late.
        row.transferred = row
            .transferred
            .max(event.transferred)
            .min(event.total.max(row.total));
        row.total = row.total.max(event.total);
        row.status = status;
        row.error = event.error;
    } else {
        snapshot.activity.push_back(ActivityRow {
            id: *next_activity_id,
            kind: event.kind,
            relative_path: event.relative_path,
            status,
            transferred: event.transferred.min(event.total.max(event.transferred)),
            total: event.total,
            error: event.error,
        });
        *next_activity_id = next_activity_id.saturating_add(1);
        while snapshot.activity.len() > MAX_CURRENT_RUN_ACTIVITY {
            snapshot.activity.pop_front();
        }
    }

    snapshot.completed = snapshot
        .activity
        .iter()
        .filter(|row| row.status == ActivityStatus::Completed)
        .count();
    let mut latest_paths = HashSet::new();
    snapshot.failed = snapshot
        .activity
        .iter()
        .rev()
        .filter(|row| latest_paths.insert((row.kind, row.relative_path.clone())))
        .filter(|row| row.status == ActivityStatus::Failed)
        .count();
    snapshot.transferred_bytes = snapshot
        .activity
        .iter()
        .filter(|row| row.id >= snapshot.batch_start_id)
        .map(|row| row.transferred)
        .sum();
    snapshot.total_bytes = snapshot
        .activity
        .iter()
        .filter(|row| row.id >= snapshot.batch_start_id)
        .map(|row| row.total)
        .sum();
    if !matches!(snapshot.work, WorkState::PausedForReconnect) {
        snapshot.work = derive_work_state(&snapshot.activity);
    }
}

fn derive_work_state(activity: &VecDeque<ActivityRow>) -> WorkState {
    let active = activity.iter().rev().find(|row| {
        matches!(
            row.status,
            ActivityStatus::Started | ActivityStatus::Progress
        )
    });
    match active.map(|row| row.kind) {
        Some(TransferKind::Upload) => WorkState::Uploading,
        Some(TransferKind::Download | TransferKind::Restore) => WorkState::Restoring,
        None => WorkState::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn transfer(state: TransferState, transferred: u64, total: u64) -> TransferEvent {
        TransferEvent {
            kind: TransferKind::Upload,
            state,
            relative_path: "daily/database.zip".to_string(),
            transferred,
            total,
            error: None,
            auth_failed: false,
        }
    }

    #[test]
    fn controller_publishes_live_monotonic_progress() {
        let (handle, events) = AppController::start(AppSnapshot::default());
        handle
            .send(AppCommand::Transfer(transfer(
                TransferState::Started,
                40,
                100,
            )))
            .unwrap();
        handle
            .send(AppCommand::Transfer(transfer(
                TransferState::Progress,
                25,
                100,
            )))
            .unwrap();
        for _ in 0..2 {
            events.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.activity.len(), 1);
        assert_eq!(snapshot.activity[0].transferred, 40);
        assert_eq!(snapshot.activity[0].percent(), 40);
        assert_eq!(snapshot.work, WorkState::Uploading);
    }

    #[test]
    fn activity_is_bounded_to_current_run() {
        let mut snapshot = AppSnapshot::default();
        let (events, _) = mpsc::channel();
        let mut next_id = 1;
        for index in 0..(MAX_CURRENT_RUN_ACTIVITY + 5) {
            let mut event = transfer(TransferState::Completed, 1, 1);
            event.relative_path = format!("{index}.zip");
            reduce(
                &mut snapshot,
                AppCommand::Transfer(event),
                &events,
                &mut next_id,
            );
        }
        assert_eq!(snapshot.activity.len(), MAX_CURRENT_RUN_ACTIVITY);
        assert_eq!(snapshot.activity.front().unwrap().relative_path, "5.zip");
    }

    #[test]
    fn auth_failure_pauses_without_clearing_activity() {
        let mut snapshot = AppSnapshot::default();
        let (events, _) = mpsc::channel();
        let mut next_id = 1;
        let mut event = transfer(TransferState::Failed, 10, 100);
        event.auth_failed = true;
        event.error = Some("Access denied".to_string());
        reduce(
            &mut snapshot,
            AppCommand::Transfer(event),
            &events,
            &mut next_id,
        );
        assert!(matches!(
            snapshot.connection,
            ConnectionState::ReconnectRequired { .. }
        ));
        assert_eq!(snapshot.work, WorkState::PausedForReconnect);
        assert_eq!(snapshot.failed, 1);
    }

    #[test]
    fn reconnect_failure_keeps_existing_connection() {
        let mut snapshot = AppSnapshot {
            connection: ConnectionState::Connected,
            ..AppSnapshot::default()
        };
        let (events, _) = mpsc::channel();
        let mut next_id = 1;
        reduce(&mut snapshot, AppCommand::Connect, &events, &mut next_id);
        reduce(
            &mut snapshot,
            AppCommand::PairFailed {
                message: "cancelled".into(),
                retryable: false,
            },
            &events,
            &mut next_id,
        );
        assert_eq!(snapshot.connection, ConnectionState::Connected);
    }

    #[test]
    fn batch_progress_excludes_completed_work_from_earlier_scans() {
        let mut snapshot = AppSnapshot::default();
        let (events, _) = mpsc::channel();
        let mut next_id = 1;
        reduce(
            &mut snapshot,
            AppCommand::Transfer(transfer(TransferState::Completed, 100, 100)),
            &events,
            &mut next_id,
        );
        reduce(
            &mut snapshot,
            AppCommand::ScanStarted,
            &events,
            &mut next_id,
        );
        let mut next = transfer(TransferState::Progress, 25, 50);
        next.relative_path = "daily/new.zip".into();
        reduce(
            &mut snapshot,
            AppCommand::Transfer(next),
            &events,
            &mut next_id,
        );
        assert_eq!(snapshot.transferred_bytes, 25);
        assert_eq!(snapshot.total_bytes, 50);
        assert_eq!(snapshot.activity.len(), 2);
    }

    #[test]
    fn successful_retry_resolves_the_latest_failure_for_a_path() {
        let mut snapshot = AppSnapshot::default();
        let (events, _) = mpsc::channel();
        let mut next_id = 1;
        let mut failed = transfer(TransferState::Failed, 10, 100);
        failed.error = Some("temporary outage".into());
        reduce(
            &mut snapshot,
            AppCommand::Transfer(failed),
            &events,
            &mut next_id,
        );
        assert_eq!(snapshot.failed, 1);
        reduce(
            &mut snapshot,
            AppCommand::ScanStarted,
            &events,
            &mut next_id,
        );
        reduce(
            &mut snapshot,
            AppCommand::Transfer(transfer(TransferState::Started, 0, 100)),
            &events,
            &mut next_id,
        );
        reduce(
            &mut snapshot,
            AppCommand::Transfer(transfer(TransferState::Completed, 100, 100)),
            &events,
            &mut next_id,
        );
        assert_eq!(snapshot.failed, 0);
    }
}
