//! Headless sync host for the macOS menubar / daemon (Option H chunk engine).

use crate::app::{
    AppCommand, AppController, AppHandle, AppSnapshot, ConnectionState, PairingState, WorkState,
};
use crate::config::{self, Config};
use crate::logs;
use crate::pairing::{self, PairStatusResponse};
use crate::sync::SyncEngine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SyncHost {
    pub config: Config,
    engine: Option<SyncEngine>,
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
            folder_label: if !config.destination_label.is_empty() {
                config.destination_label.clone()
            } else {
                config.syncthing_folder_label.clone()
            },
            ..AppSnapshot::default()
        };
        let (app, events) = AppController::start(initial);
        std::thread::spawn(move || while events.recv().is_ok() {});
        Self {
            config,
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
        if let Some(engine) = self.engine.take() {
            engine.stop();
        }
    }

    pub fn restart_sync(&mut self) -> Result<(), String> {
        self.stop_sync();
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

        let engine = SyncEngine::start(self.config.clone()).inspect_err(|error| {
            let _ = self.app.send(AppCommand::EngineFailed(error.clone()));
        })?;
        self.reconnect_required.store(false, Ordering::Relaxed);
        self.engine = Some(engine);
        logs::append(&format!(
            "Chunk sync started: destination={} endpoint={}",
            self.config.destination_uuid, self.config.chunk_endpoint
        ));
        Ok(())
    }

    /// POST `/pair/start` for Option H chunk_store pairing.
    pub fn pair_start_request(&self) -> Result<pairing::PairStartResponse, String> {
        if !watch_folder_is_valid(&self.config.watch_folder) {
            return Err("Set a valid watch folder before pairing.".into());
        }
        let api_base = self.config.pair_api_base.clone();
        let machine_name = host_machine_name();
        let user_name = host_user_name();
        let backup_path = self.config.watch_folder.clone();
        let suggested_customer = pairing::build_host_folder_hint(&machine_name, &backup_path);
        logs::append(&format!(
            "Pair start: machine={machine_name} user={user_name} backup={backup_path} transport=chunk_store suggested={}",
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

    /// Persist chunk_store approval and start the in-process sync engine.
    pub fn pair_apply_and_sync(&mut self, status: PairStatusResponse) -> Result<(), String> {
        let _ = self.app.send(AppCommand::PairApproved);
        self.stop_sync();
        self.apply_pair_approval(status)?;
        self.restart_sync()?;
        logs::append("Pairing complete; chunk sync started.");
        Ok(())
    }

    fn apply_pair_approval(&mut self, status: PairStatusResponse) -> Result<(), String> {
        if !pairing::is_chunk_store_approval(&status) {
            return Err("Pairing approved without a chunk_store assignment. Pair again.".into());
        }
        let device_token = required_field(status.device_token, "device token")?;
        let device_uuid = required_field(status.device_uuid, "device UUID")?;
        let destination_uuid = required_field(status.destination_uuid, "destination UUID")?;
        let destination_label = required_field(status.destination_label, "destination label")?;
        let chunk_endpoint = required_field(status.chunk_endpoint, "chunk endpoint")?;
        let chunk_bucket = required_field(status.chunk_bucket, "chunk bucket")?;
        let chunk_access_key = required_field(status.chunk_access_key, "chunk access key")?;
        let chunk_secret_key = required_field(status.chunk_secret_key, "chunk secret key")?;

        let mut candidate = self.config.clone();
        candidate.schema_version = config::CONFIG_SCHEMA_VERSION;
        candidate.device_uuid = device_uuid;
        candidate.destination_uuid = destination_uuid;
        candidate.destination_label = destination_label.clone();
        candidate.transport = "chunk_store".into();
        candidate.chunk_endpoint = chunk_endpoint;
        candidate.chunk_region = status.chunk_region.unwrap_or_else(|| "garage".into());
        candidate.chunk_bucket = chunk_bucket;
        candidate.chunk_prefix = status.chunk_prefix.unwrap_or_default();
        candidate.chunk_path_style = status.chunk_path_style.unwrap_or(true);
        candidate.syncthing_folder_label = destination_label;
        candidate.server_approved_at = Some(approval_timestamp_now());
        candidate = config::save_pairing_candidate(
            candidate,
            &device_token,
            &chunk_access_key,
            &chunk_secret_key,
        )?;
        self.config = candidate;
        self.engine = None;
        self.reconnect_required.store(false, Ordering::Relaxed);
        Ok(())
    }
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
