//! Headless sync host for macOS menubar / daemon (no HWND).

use crate::config::{self, Config, TransportKind};
use crate::logs;
use crate::pairing::{self, PairStatusResponse};
use crate::secret;
use crate::sync::{self, ActivityFn, ActivityInfo, ActivityState, AuthFailedFn, LogFn, SyncEngine};
use crate::transport;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SyncHost {
    pub config: Config,
    pub s3_secret_plain: String,
    engine: Option<SyncEngine>,
    auth_failed: Arc<AtomicBool>,
}

impl SyncHost {
    pub fn load() -> Self {
        let config = config::load();
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
        Self {
            config,
            s3_secret_plain,
            engine: None,
            auth_failed: Arc::new(AtomicBool::new(false)),
        }
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
        logs::append(&format!("Watch folder set: {}", path.display()));
        if self.is_configured() {
            self.restart_sync()?;
        }
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
        let engine = SyncEngine::start(
            self.config.clone(),
            transport,
            log,
            activity,
            auth_failed,
        )?;
        self.engine = Some(engine);
        let msg = format!("Sync engine started for {}", self.config.watch_folder);
        logs::append(&msg);
        eprintln!("{msg}");
        Ok(())
    }

    /// Blocking pair flow (no XD). Calls `on_status` for UI/tooltip updates.
    pub fn pair_blocking<F>(&mut self, mut on_status: F) -> Result<(), String>
    where
        F: FnMut(&str),
    {
        if !watch_folder_is_valid(&self.config.watch_folder) {
            let msg = "Set a valid watch folder before pairing.";
            crate::macos::notify::pair_watch_folder_required();
            on_status(msg);
            return Err(msg.into());
        }

        let api_base = self.config.pair_api_base.clone();
        let machine = host_machine_name();
        let user = host_user_name();
        let version = env!("CARGO_PKG_VERSION");

        let start = pairing::start_pairing(
            &api_base,
            &machine,
            &user,
            version,
            None,
            None,
            None,
            None,
            None,
        )
        .ok_or_else(|| {
            let msg = "Pair start failed (network or server error).";
            crate::macos::notify::pair_failed(msg);
            msg.to_string()
        })?;

        eprintln!("Pairing code: {}", start.code);
        eprintln!("Approve URL:  {}", start.approve_url);
        eprintln!("Waiting for server approval...");
        let waiting = format!(
            "Pairing… code {} — waiting for approval",
            start.code
        );
        on_status(&waiting);
        crate::macos::notify::pair_started(&start.code, &start.approve_url);

        let sleep_ms = start.poll_interval_ms.clamp(1000, 10_000);
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            if Instant::now() > deadline {
                let msg = format!(
                    "Pairing timed out.\nCode: {}\nApprove URL: {}",
                    start.code, start.approve_url
                );
                crate::macos::notify::pair_failed(&msg);
                on_status(&msg);
                return Err(msg);
            }
            std::thread::sleep(Duration::from_millis(sleep_ms));
            let Some(status) = pairing::poll_pairing(&api_base, &start.poll_token) else {
                on_status(&waiting);
                continue;
            };
            match status.status.as_str() {
                "approved" => {
                    if let Err(err) = self.apply_pair_approval(status) {
                        crate::macos::notify::pair_failed(&err);
                        return Err(err);
                    }
                    self.auth_failed.store(false, Ordering::Relaxed);
                    if let Err(err) = self.restart_sync() {
                        crate::macos::notify::pair_failed(&err);
                        return Err(err);
                    }
                    logs::append("Pairing complete; initial sync started.");
                    eprintln!("Paired. Sync started.");
                    on_status("Paired — sync started");
                    crate::macos::notify::pair_finished();
                    return Ok(());
                }
                "rejected" => {
                    let msg = "Pairing rejected by server.";
                    crate::macos::notify::pair_failed(msg);
                    return Err(msg.into());
                }
                "expired" => {
                    let msg = "Pairing code expired. Try again.";
                    crate::macos::notify::pair_failed(msg);
                    return Err(msg.into());
                }
                "pending" | "waiting" => {
                    on_status(&waiting);
                    eprint!(".");
                    continue;
                }
                other => {
                    logs::append(&format!("Pair poll status: {other}"));
                    on_status(&waiting);
                    continue;
                }
            }
        }
    }

    pub fn restore_blocking(&mut self, destination_parent: &Path) -> Result<PathBuf, String> {
        if self.auth_failed.load(Ordering::Relaxed) {
            return Err("Cannot restore until credentials are reconnected (pair again).".into());
        }
        if !is_sync_configured(&self.config, &self.s3_secret_plain) {
            return Err("Cannot restore: sync is not configured.".into());
        }
        if !destination_parent.is_dir() {
            return Err(format!(
                "destination parent is not a directory: {}",
                destination_parent.display()
            ));
        }
        let transport = transport::build(&self.config, &self.s3_secret_plain)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (log, activity, auth_failed) = self.callbacks();
        eprintln!("Restoring customer backup...");
        let path = sync::restore_customer_backup(
            &self.config,
            transport,
            destination_parent,
            &cancel,
            &log,
            &activity,
            &auth_failed,
        )?;
        eprintln!("Restore saved to {}", path.display());
        Ok(path)
    }

    fn apply_pair_approval(&mut self, status: PairStatusResponse) -> Result<(), String> {
        if !pairing::is_s3_approval(&status) {
            return Err(
                "Pairing approved without S3 credentials. Pair again after the server enables S3."
                    .into(),
            );
        }
        let device_token = required_field(status.device_token, "device token")?;
        let remote_folder = approved_remote_folder(status.remote_folder.as_deref())?;
        let s3_endpoint = required_field(status.s3_endpoint, "S3 endpoint")?;
        let s3_bucket = required_field(status.s3_bucket, "S3 bucket")?;
        let s3_access_key = required_field(status.s3_access_key, "S3 access key")?;
        let s3_secret_key = required_field(status.s3_secret_key, "S3 secret key")?;

        let mut cfg = self.config.clone();
        cfg.device_token_enc = secret::protect("device_token", &device_token)?;
        cfg.s3_secret_enc = secret::protect("s3_secret", &s3_secret_key)?;
        cfg.schema_version = 2;
        cfg.device_uuid = status.device_uuid.unwrap_or_default();
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

        config::save(&cfg).map_err(|e| format!("Pairing succeeded but save failed: {e}"))?;
        self.s3_secret_plain = s3_secret_key;
        self.config = cfg;
        Ok(())
    }

    fn callbacks(&self) -> (LogFn, ActivityFn, AuthFailedFn) {
        let auth_flag = self.auth_failed.clone();
        let log: LogFn = Arc::new(|m: String| {
            logs::append(&m);
            eprintln!("{m}");
        });
        let activity: ActivityFn = Arc::new(move |info: ActivityInfo| {
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

fn approved_remote_folder(value: Option<&str>) -> Result<String, String> {
    let folder = value.unwrap_or("").trim();
    if folder.is_empty() {
        return Err("Pairing approved without a destination folder.".into());
    }
    Ok(folder.to_string())
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
