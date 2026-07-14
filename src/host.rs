//! Headless sync host for macOS menubar / daemon (no HWND).

use crate::app::{
    AppCommand, AppController, AppHandle, AppSnapshot, ConnectionState, PairingState, WorkState,
};
use crate::config::{self, Config, TransportKind};
use crate::logs;
use crate::pairing::{self, PairStatusResponse};
use crate::secret;
use crate::sync::{self, ActivityFn, ActivityInfo, ActivityState, AuthFailedFn, LogFn, SyncEngine};
use crate::transport;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SyncHost {
    pub config: Config,
    pub s3_secret_plain: String,
    engine: Option<SyncEngine>,
    auth_failed: Arc<AtomicBool>,
    app: AppHandle,
}

impl SyncHost {
    pub fn load() -> Self {
        let config = config::load();
        #[cfg(target_os = "macos")]
        secret::purge_stale_keychain_handles(&[&config.s3_secret_enc, &config.device_token_enc]);
        let s3_secret_plain = if config.s3_secret_enc.trim().is_empty() {
            String::new()
        } else {
            match secret::decrypt(&config.s3_secret_enc) {
                Ok(s) => s,
                Err(err) => {
                    logs::append(&format!("Could not decrypt S3 secret: {err}"));
                    String::new()
                }
            }
        };
        if !s3_secret_plain.is_empty() {
            logs::register_secret(&s3_secret_plain);
        }
        logs::register_secret(&config.s3_access_key);
        let configured = is_sync_configured(&config, &s3_secret_plain);
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
            ..AppSnapshot::default()
        };
        let (app, events) = AppController::start(initial);
        std::thread::spawn(move || while events.recv().is_ok() {});
        Self {
            config,
            s3_secret_plain,
            engine: None,
            auth_failed: Arc::new(AtomicBool::new(false)),
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
        !self.config.device_token_enc.trim().is_empty()
    }

    pub fn is_configured(&self) -> bool {
        is_sync_configured(&self.config, &self.s3_secret_plain)
    }

    pub fn auth_failed(&self) -> bool {
        self.auth_failed.load(Ordering::Relaxed)
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
        config::save(&self.config).map_err(|e| format!("save config: {e}"))?;
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
        config::save(&self.config).map_err(|e| format!("save config: {e}"))?;
        let _ = self
            .app
            .send(AppCommand::SetPairApiBase(normalized.clone()));
        logs::append(&format!("Control plane URL set: {normalized}"));
        Ok(())
    }

    pub fn stop_sync(&mut self) {
        self.engine = None;
    }

    pub fn restart_sync(&mut self) -> Result<(), String> {
        self.engine = None;
        if self.auth_failed.load(Ordering::Relaxed) {
            return Err("Sync paused: credentials failed. Pair again.".into());
        }
        if !watch_folder_is_valid(&self.config.watch_folder) {
            return Err("Sync not started: choose a valid watch folder.".into());
        }
        if !is_sync_configured(&self.config, &self.s3_secret_plain) {
            return Err(
                "Sync not started: watch folder, S3 credentials, and destination required.".into(),
            );
        }
        let transport = transport::build(&self.config, &self.s3_secret_plain)?;
        let (log, activity, auth_failed) = self.callbacks();
        let engine = SyncEngine::start_with_events(
            self.config.clone(),
            transport,
            log,
            activity,
            auth_failed,
            self.app.transfer_event_callback(),
        )?;
        self.engine = Some(engine);
        let msg = format!("Sync engine started for {}", self.config.watch_folder);
        logs::append(&msg);
        eprintln!("{msg}");
        Ok(())
    }

    /// POST /pair/start (no XD). Caller owns UI; keep host mutex unlocked during poll.
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
            "Pair start: machine={machine_name} user={user_name} backup={backup_path} suggested={}",
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
        .map_err(|err| err.to_string());
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

    /// Persist approval + start sync. Hold host mutex only for this short step.
    pub fn pair_apply_and_sync(&mut self, status: PairStatusResponse) -> Result<(), String> {
        let _ = self.app.send(AppCommand::PairApproved);
        if let Err(err) = self.apply_pair_approval(status) {
            if self.is_paired() {
                self.auth_failed.store(true, Ordering::Relaxed);
                let _ = self.app.send(AppCommand::AuthFailed(format!(
                    "Approved reconnect could not be activated: {err}"
                )));
            }
            return Err(err);
        }
        self.auth_failed.store(false, Ordering::Relaxed);
        self.restart_sync()?;
        let _ = self.app.send(AppCommand::ConnectionValidated);
        logs::append("Pairing complete; initial sync started.");
        eprintln!("Paired. Sync started.");
        Ok(())
    }

    pub fn start_restore_job(
        &self,
        destination_parent: PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<std::sync::mpsc::Receiver<Result<PathBuf, String>>, String> {
        if self.auth_failed.load(Ordering::Relaxed) {
            return Err("Cannot restore until credentials are reconnected (pair again).".into());
        }
        if !is_sync_configured(&self.config, &self.s3_secret_plain) {
            return Err("Cannot restore: sync is not configured.".into());
        }
        let cfg = self.config.clone();
        let transport = transport::build(&cfg, &self.s3_secret_plain)?;
        let (log, activity, auth_failed) = self.callbacks();
        let events = self.app.transfer_event_callback();
        let app = self.app.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = sync::restore_customer_backup_with_events(
                &cfg,
                transport,
                &destination_parent,
                &cancel,
                &log,
                &activity,
                &auth_failed,
                events,
            );
            let _ = app.send(AppCommand::ScanFinished);
            let _ = tx.send(result);
        });
        Ok(rx)
    }

    pub fn retry_failed_uploads(&mut self) -> Result<usize, String> {
        if self.auth_failed.load(Ordering::Relaxed) {
            return Err("Reconnect before retrying uploads.".into());
        }
        let snapshot = self.app.snapshot();
        let mut latest_paths = HashSet::new();
        let failed: Vec<String> = snapshot
            .activity
            .iter()
            .rev()
            .filter(|row| latest_paths.insert((row.kind, row.relative_path.clone())))
            .filter(|row| row.status == crate::app::ActivityStatus::Failed)
            .map(|row| row.relative_path.clone())
            .collect();
        if failed.is_empty() {
            return Ok(0);
        }
        let transport = transport::build(&self.config, &self.s3_secret_plain)?;
        let (log, activity, auth_failed) = self.callbacks();
        let result = sync::retry_uploads_with_events(
            &self.config,
            transport,
            &failed,
            &log,
            &activity,
            &auth_failed,
            self.app.transfer_event_callback(),
        );
        Ok(result.attempted)
    }

    fn apply_pair_approval(&mut self, status: PairStatusResponse) -> Result<(), String> {
        if !pairing::is_s3_approval(&status) {
            return Err(
                "Pairing approved without S3 credentials. Pair again after the server enables S3."
                    .into(),
            );
        }
        let device_token = required_field(status.device_token, "device token")?;
        let device_uuid = required_field(status.device_uuid, "device UUID")?;
        let s3_endpoint = required_field(status.s3_endpoint, "S3 endpoint")?;
        let s3_bucket =
            pairing::validate_destination_name(&required_field(status.s3_bucket, "S3 bucket")?)?;
        let remote_folder = pairing::validate_destination_name(&required_field(
            status.remote_folder,
            "customer destination",
        )?)?;
        let s3_access_key = required_field(status.s3_access_key, "S3 access key")?;
        let s3_secret_key = required_field(status.s3_secret_key, "S3 secret key")?;

        let mut cfg = self.config.clone();
        cfg.schema_version = 2;
        cfg.device_uuid = device_uuid;
        cfg.transport = "s3".to_string();
        cfg.s3_endpoint = s3_endpoint;
        cfg.s3_region = status
            .s3_region
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| "garage".to_string());
        cfg.s3_bucket = s3_bucket;
        cfg.s3_access_key = s3_access_key;
        cfg.s3_path_style = status.s3_path_style.unwrap_or(true);
        cfg.s3_prefix = status.s3_prefix.unwrap_or_default();
        cfg.parallel_uploads = 2;
        cfg.remote_folder = remote_folder;
        cfg.server_approved_at = Some(approval_timestamp_now());
        cfg.credential_profile_id = status.credential_profile_id;
        cfg.credential_version = status.credential_version;

        transport::validate_candidate(&cfg, &s3_secret_key)?;
        cfg = config::save_pairing_candidate(cfg, &device_token, &s3_secret_key)?;
        logs::register_secret(&device_token);
        logs::register_secret(&s3_secret_key);
        logs::register_secret(&cfg.s3_access_key);
        self.s3_secret_plain = s3_secret_key;
        self.config = cfg;
        Ok(())
    }

    fn callbacks(&self) -> (LogFn, ActivityFn, AuthFailedFn) {
        let auth_flag = self.auth_failed.clone();
        let activity_app = self.app.clone();
        let auth_app = self.app.clone();
        let log: LogFn = Arc::new(|m: String| {
            logs::append(&m);
            eprintln!("{m}");
        });
        let activity: ActivityFn = Arc::new(move |info: ActivityInfo| {
            let command = match info.state {
                ActivityState::Checking | ActivityState::Syncing => AppCommand::ScanStarted,
                ActivityState::Idle => AppCommand::ScanFinished,
            };
            let _ = activity_app.send(command);
            let state = match info.state {
                ActivityState::Checking => "checking",
                ActivityState::Syncing => "syncing",
                ActivityState::Idle => "idle",
            };
            eprintln!(
                "activity: {state} {}/{} failed={}",
                info.completed, info.total, info.failed
            );
            if !info.failed_paths.is_empty() {
                for p in info.failed_paths.iter().take(5) {
                    eprintln!("  fail: {p}");
                }
            }
        });
        let auth_failed: AuthFailedFn = Arc::new(move || {
            auth_flag.store(true, Ordering::Relaxed);
            let _ = auth_app.send(AppCommand::AuthFailed(
                "S3 credentials or policy were rejected.".into(),
            ));
            logs::append("S3 auth/policy failure — re-pair required");
            eprintln!("AUTH FAILED — pair again");
        });
        (log, activity, auth_failed)
    }
}

fn is_sync_configured(cfg: &Config, s3_secret: &str) -> bool {
    if !watch_folder_is_valid(&cfg.watch_folder) || cfg.remote_folder.trim().is_empty() {
        return false;
    }
    matches!(config::transport_kind(cfg), Some(TransportKind::S3))
        && !cfg.s3_endpoint.trim().is_empty()
        && !cfg.s3_bucket.trim().is_empty()
        && !cfg.s3_access_key.trim().is_empty()
        && !s3_secret.is_empty()
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
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

fn host_machine_name() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return h;
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    "macOS".to_string()
}

fn host_user_name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "macuser".to_string())
}
