//! Headless Syncthing host for the macOS menubar / daemon.

use crate::app::{
    AppCommand, AppController, AppHandle, AppSnapshot, ConnectionState, PairingState, WorkState,
};
use crate::config::{self, Config};
use crate::logs;
use crate::pairing::{self, PairStatusResponse};
use crate::syncthing::{FolderAssignment, SyncthingMonitor, SyncthingSupervisor};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SyncHost {
    pub config: Config,
    monitor: Option<SyncthingMonitor>,
    engine: Option<SyncthingSupervisor>,
    reconnect_required: Arc<AtomicBool>,
    app: AppHandle,
}

impl SyncHost {
    pub fn load() -> Self {
        let config = config::load();
        #[cfg(target_os = "macos")]
        crate::secret::purge_stale_keychain_handles(&[&config.device_token_enc]);
        let configured = is_sync_configured(&config);
        let initial = AppSnapshot {
            connection: if configured {
                ConnectionState::Connected
            } else {
                ConnectionState::Disconnected
            },
            pairing: PairingState::Idle,
            work: WorkState::Idle,
            watch_folder: (!config.watch_folder.trim().is_empty())
                .then(|| PathBuf::from(&config.watch_folder)),
            pair_api_base: config.pair_api_base.clone(),
            start_at_login: config.start_with_windows,
            auto_update: config.auto_update,
            folder_label: config.syncthing_folder_label.clone(),
            ..AppSnapshot::default()
        };
        let (app, events) = AppController::start(initial);
        std::thread::spawn(move || while events.recv().is_ok() {});
        Self {
            config,
            monitor: None,
            engine: None,
            reconnect_required: Arc::new(AtomicBool::new(false)),
            app,
        }
    }

    pub fn app_snapshot(&self) -> AppSnapshot {
        self.app.snapshot()
    }

    pub fn cancel_pairing(&self) {
        let _ = self.app.send(AppCommand::CancelPairing);
    }

    pub fn pairing_failed(&self, message: String, retryable: bool) {
        let _ = self.app.send(AppCommand::PairFailed { message, retryable });
    }

    pub fn is_paired(&self) -> bool {
        config::is_paired(&self.config)
    }

    pub fn is_configured(&self) -> bool {
        is_sync_configured(&self.config)
    }

    pub fn auth_failed(&self) -> bool {
        self.reconnect_required.load(Ordering::Relaxed)
    }

    pub fn engine_running(&self) -> bool {
        self.engine.is_some()
    }

    pub fn set_watch_folder(&mut self, path: PathBuf) -> Result<(), String> {
        if !path.is_absolute() {
            return Err("watch folder must be an absolute path".into());
        }
        if !path.is_dir() {
            return Err(format!("not an existing directory: {}", path.display()));
        }
        self.config.watch_folder = path.display().to_string();
        config::save(&self.config).map_err(|error| format!("save config: {error}"))?;
        let _ = self.app.send(AppCommand::SetWatchFolder(path.clone()));
        logs::append(&format!("Watch folder set: {}", path.display()));
        if self.is_configured() {
            self.restart_sync()?;
        }
        Ok(())
    }

    pub fn set_pair_api_base(&mut self, raw: &str) -> Result<(), String> {
        let normalized = config::normalize_pair_api_base(raw)?;
        self.config.pair_api_base = normalized.clone();
        config::save(&self.config).map_err(|error| format!("save config: {error}"))?;
        let _ = self
            .app
            .send(AppCommand::SetPairApiBase(normalized.clone()));
        logs::append(&format!("Control plane URL set: {normalized}"));
        Ok(())
    }

    pub fn stop_sync(&mut self) {
        self.monitor = None;
        self.engine = None;
    }

    pub fn restart_sync(&mut self) -> Result<(), String> {
        self.monitor = None;
        self.engine = None;
        let _ = self.app.send(AppCommand::EngineStarting);
        if !watch_folder_is_valid(&self.config.watch_folder) {
            let error = "Sync not started: choose a valid watch folder.".to_string();
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
            return Err(error);
        }
        if !config::is_paired(&self.config) {
            let error = "Sync not started: pair this computer again.".to_string();
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
            return Err(error);
        }
        crate::paths::validate_bundled_engine_installation().inspect_err(|error| {
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
        })?;

        let engine = SyncthingSupervisor::start().inspect_err(|error| {
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
        })?;
        let assignment = assignment_from_config(&self.config).inspect_err(|error| {
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
        })?;
        if let Err(error) = engine.configure_folder(&assignment) {
            self.reconnect_required.store(true, Ordering::Relaxed);
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
            return Err(error);
        }
        let status = engine
            .status(
                &self.config.syncthing_folder_id,
                &self.config.syncthing_hub_device_id,
            )
            .inspect_err(|error| {
                let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
            })?;
        self.reconnect_required.store(false, Ordering::Relaxed);
        let _ = self.app.send(AppCommand::SyncthingStatus(status.clone()));
        let status_app = self.app.clone();
        let events_app = self.app.clone();
        let failure_app = self.app.clone();
        let status_reconnect = self.reconnect_required.clone();
        let failure_reconnect = self.reconnect_required.clone();
        let monitor = engine.start_monitor(
            self.config.syncthing_folder_id.clone(),
            self.config.syncthing_hub_device_id.clone(),
            self.app.snapshot().last_event_id,
            move |status| {
                status_reconnect.store(false, Ordering::Relaxed);
                let _ = status_app.send(AppCommand::SyncthingStatus(status));
            },
            move |events| {
                let _ = events_app.send(AppCommand::SyncthingEvents(events));
            },
            move |error| {
                failure_reconnect.store(true, Ordering::Relaxed);
                let _ = failure_app.send(AppCommand::EngineFailed(error));
            },
        );
        self.monitor = Some(monitor);
        self.engine = Some(engine);
        logs::append(&format!(
            "Syncthing started: folder={} state={} hub_connected={} need_files={} need_bytes={}",
            self.config.syncthing_folder_id,
            status.folder_state,
            status.hub_connected,
            status.need_files,
            status.need_bytes
        ));
        Ok(())
    }

    /// POST `/pair/start` after the private engine has generated its stable
    /// certificate-derived device identity.
    pub fn pair_start_request(&self) -> Result<pairing::PairStartResponse, String> {
        if !watch_folder_is_valid(&self.config.watch_folder) {
            return Err("Set a valid watch folder before pairing.".into());
        }
        crate::paths::validate_bundled_engine_installation()?;
        let syncthing_device_id = match self.engine.as_ref() {
            Some(engine) => engine.device_id()?,
            None => crate::syncthing::ensure_local_device_id()?,
        };
        let api_base = self.config.pair_api_base.clone();
        let machine_name = host_machine_name();
        let user_name = host_user_name();
        let backup_path = self.config.watch_folder.clone();
        let suggested_customer = pairing::build_host_folder_hint(&machine_name, &backup_path);
        logs::append(&format!(
            "Pair start: machine={machine_name} user={user_name} backup={backup_path} syncthing_device_id={syncthing_device_id} suggested={}",
            suggested_customer.as_deref().unwrap_or("none")
        ));
        let _ = self.app.send(AppCommand::Connect);
        let result = pairing::start_pairing_result(
            &api_base,
            &machine_name,
            &user_name,
            env!("CARGO_PKG_VERSION"),
            None,
            Some(backup_path),
            None,
            None,
            suggested_customer,
            syncthing_device_id,
        )
        .map_err(|error| error.to_string());
        match &result {
            Ok(start) => {
                let _ = self.app.send(AppCommand::PairStarted {
                    code: start.code.clone(),
                    approve_url: start.approve_url.clone(),
                    api_base,
                });
            }
            Err(message) => {
                let _ = self.app.send(AppCommand::PairFailed {
                    message: message.clone(),
                    retryable: true,
                });
            }
        }
        result
    }

    /// Validate the approved hub assignment locally, persist it, and start the
    /// long-running engine. No object-storage credential is accepted.
    pub fn pair_apply_and_sync(&mut self, status: PairStatusResponse) -> Result<(), String> {
        let _ = self.app.send(AppCommand::PairApproved);
        // Re-pairing can happen while the previous assignment is live. Stop
        // the owned process first so approval validation cannot attach a
        // second supervisor to the same private loopback instance.
        self.stop_sync();
        self.apply_pair_approval(status)?;
        self.restart_sync()?;
        logs::append("Pairing complete; Syncthing started.");
        Ok(())
    }

    fn apply_pair_approval(&mut self, status: PairStatusResponse) -> Result<(), String> {
        if !pairing::is_syncthing_approval(&status) {
            return Err("Pairing approved without a Syncthing assignment. Pair again.".into());
        }
        let device_token = required_field(status.device_token, "device token")?;
        let device_uuid = required_field(status.device_uuid, "device UUID")?;
        let hub_device_id =
            required_field(status.syncthing_hub_device_id, "Syncthing hub device ID")?;
        let folder_id = required_field(status.syncthing_folder_id, "Syncthing folder ID")?;
        let folder_label = required_field(status.syncthing_folder_label, "Syncthing folder label")?;
        let engine = SyncthingSupervisor::start()?;
        let local_device_id = engine.device_id()?;
        let assignment = FolderAssignment {
            local_device_id: local_device_id.clone(),
            hub_device_id: hub_device_id.clone(),
            hub_addresses: status.syncthing_hub_addresses.clone(),
            folder_id: folder_id.clone(),
            folder_label: folder_label.clone(),
            path: PathBuf::from(&self.config.watch_folder),
        };
        engine.configure_folder(&assignment)?;

        let mut candidate = self.config.clone();
        candidate.schema_version = config::CONFIG_SCHEMA_VERSION;
        candidate.device_uuid = device_uuid;
        candidate.syncthing_device_id = local_device_id;
        candidate.syncthing_hub_device_id = hub_device_id;
        candidate.syncthing_hub_addresses = status.syncthing_hub_addresses;
        candidate.syncthing_folder_id = folder_id;
        candidate.syncthing_folder_label = folder_label;
        candidate.server_approved_at = Some(approval_timestamp_now());
        candidate = config::save_pairing_candidate(candidate, &device_token)?;
        self.config = candidate;
        self.monitor = None;
        self.engine = Some(engine);
        self.reconnect_required.store(false, Ordering::Relaxed);
        Ok(())
    }
}

fn assignment_from_config(config: &Config) -> Result<FolderAssignment, String> {
    let assignment = FolderAssignment {
        local_device_id: config.syncthing_device_id.clone(),
        hub_device_id: config.syncthing_hub_device_id.clone(),
        hub_addresses: config.syncthing_hub_addresses.clone(),
        folder_id: config.syncthing_folder_id.clone(),
        folder_label: config.syncthing_folder_label.clone(),
        path: PathBuf::from(&config.watch_folder),
    };
    assignment.validate()?;
    Ok(assignment)
}

fn is_sync_configured(config: &Config) -> bool {
    watch_folder_is_valid(&config.watch_folder) && config::is_paired(config)
}

fn watch_folder_is_valid(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty() && Path::new(path).is_dir()
}

fn required_field(value: Option<String>, label: &str) -> Result<String, String> {
    let value = value.unwrap_or_default();
    if value.trim().is_empty() {
        Err(format!("Pairing approved without {label}."))
    } else {
        Ok(value)
    }
}

fn approval_timestamp_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

fn host_machine_name() -> String {
    if let Ok(host) = std::env::var("HOSTNAME") {
        if !host.trim().is_empty() {
            return host;
        }
    }
    if let Ok(output) = std::process::Command::new("hostname").output() {
        let host = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !host.is_empty() {
            return host;
        }
    }
    "macOS".to_string()
}

fn host_user_name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "macuser".to_string())
}
