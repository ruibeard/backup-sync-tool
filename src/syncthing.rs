//! Supervisor and loopback REST adapter for the bundled Syncthing engine.
//!
//! The GUI/API is deliberately bound to a private, fixed loopback port. The
//! API key remains in Syncthing's own private `config.xml`; it is never sent to
//! the control plane or written to `backupsynctool.json`.

use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const GUI_ADDRESS: &str = "127.0.0.1:8385";
const API_BASE: &str = "http://127.0.0.1:8385/rest";
const START_TIMEOUT: Duration = Duration::from_secs(25);
const REQUIRED_ENGINE_VERSION: &str = "v2.1.1";

#[derive(Debug, Clone)]
pub struct FolderAssignment {
    pub local_device_id: String,
    pub hub_device_id: String,
    pub hub_addresses: Vec<String>,
    pub folder_id: String,
    pub folder_label: String,
    pub path: PathBuf,
}

impl FolderAssignment {
    pub fn validate(&self) -> Result<(), String> {
        validate_device_id(&self.local_device_id, "local Syncthing device ID")?;
        validate_device_id(&self.hub_device_id, "hub Syncthing device ID")?;
        if self
            .local_device_id
            .eq_ignore_ascii_case(&self.hub_device_id)
        {
            return Err(
                "The approved hub has the same Syncthing device ID as this computer.".into(),
            );
        }
        validate_folder_id(&self.folder_id)?;
        if self.folder_label.trim().is_empty()
            || self.folder_label.len() > 255
            || self.folder_label.chars().any(char::is_control)
        {
            return Err("Pairing approved with an invalid Syncthing folder label.".into());
        }
        if !self.path.is_absolute() || !self.path.is_dir() {
            return Err(format!(
                "The selected sync folder is not an existing absolute directory: {}",
                self.path.display()
            ));
        }
        if self.hub_addresses.is_empty() {
            return Err("Pairing approved without a Syncthing hub address.".into());
        }
        for address in &self.hub_addresses {
            validate_hub_address(address)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncStatus {
    pub hub_connected: bool,
    pub folder_state: String,
    pub local_files: u64,
    pub global_files: u64,
    pub need_files: u64,
    pub need_bytes: u64,
}

impl SyncStatus {
    pub fn is_idle(&self) -> bool {
        self.need_files == 0
            && self.need_bytes == 0
            && matches!(self.folder_state.as_str(), "idle" | "")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncthingEvent {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

pub struct SyncthingSupervisor {
    child: Option<Child>,
    client: SyncthingClient,
}

pub struct SyncthingMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SyncthingSupervisor {
    /// Start the private engine, or attach to the already-running private
    /// instance when a previous app process left it alive briefly.
    pub fn start() -> Result<Self, String> {
        let home = crate::paths::syncthing_home_dir();
        crate::paths::ensure_dir(&home)
            .map_err(|error| format!("Could not create Syncthing state directory: {error}"))?;

        if let Ok(client) = SyncthingClient::from_home(&home) {
            if client.system_status().is_ok() {
                client.validate_version()?;
                client.secure_gui()?;
                return Ok(Self {
                    child: None,
                    client,
                });
            }
        }

        let binary = crate::paths::syncthing_binary_path();
        if !binary.is_file() {
            return Err(format!(
                "Bundled Syncthing engine is missing: {}",
                binary.display()
            ));
        }
        let mut command = syncthing_command(&binary, &home);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not launch bundled Syncthing engine {}: {error}",
                binary.display()
            )
        })?;
        forward_engine_output(child.stdout.take(), "syncthing");
        forward_engine_output(child.stderr.take(), "syncthing stderr");

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("Could not inspect Syncthing engine: {error}"))?
            {
                return Err(format!(
                    "Syncthing engine exited during startup ({status})."
                ));
            }
            let startup_error = match SyncthingClient::from_home(&home).and_then(|client| {
                client.system_status()?;
                client.validate_version()?;
                client.secure_gui()?;
                Ok(client)
            }) {
                Ok(client) => {
                    crate::logs::append(
                        "Bundled Syncthing engine is ready on private loopback API.",
                    );
                    return Ok(Self {
                        child: Some(child),
                        client,
                    });
                }
                Err(error) => error,
            };
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Syncthing engine did not become ready within {} seconds: {}",
                    START_TIMEOUT.as_secs(),
                    startup_error
                ));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn device_id(&self) -> Result<String, String> {
        self.client.device_id()
    }

    pub fn configure_folder(&self, assignment: &FolderAssignment) -> Result<(), String> {
        assignment.validate()?;
        let actual_id = self.device_id()?;
        if !actual_id.eq_ignore_ascii_case(&assignment.local_device_id) {
            return Err(format!(
                "Local Syncthing identity changed (paired {}, running {}). Pair again.",
                assignment.local_device_id, actual_id
            ));
        }
        self.client.configure_folder(assignment)?;
        self.client.scan_folder(&assignment.folder_id)?;
        Ok(())
    }

    pub fn status(&self, folder_id: &str, hub_device_id: &str) -> Result<SyncStatus, String> {
        self.client.sync_status(folder_id, hub_device_id)
    }

    pub fn poll_events(
        &self,
        since: u64,
        timeout_seconds: u64,
    ) -> Result<Vec<SyncthingEvent>, String> {
        self.client.poll_events(since, timeout_seconds)
    }

    /// Explicitly stop the private instance and wait until its loopback API is
    /// gone. This works for both an owned child and a supervisor attached to
    /// the same private home, which lets the updater prove `syncthing.exe` is
    /// unlocked before replacing the tested app/engine bundle.
    pub fn shutdown(&self) -> Result<(), String> {
        self.client.shutdown_and_wait()
    }

    /// Stop an existing private instance without launching an engine. Missing
    /// state or a genuinely unavailable loopback API is already safe for
    /// bundle repair; unreadable state and a running engine that will not stop
    /// are surfaced as errors.
    pub fn shutdown_if_running() -> Result<(), String> {
        let home = crate::paths::syncthing_home_dir();
        let config = home.join("config.xml");
        if !config.is_file() {
            return Ok(());
        }
        let client = SyncthingClient::from_home(&home).map_err(|error| {
            format!("Could not read private Syncthing state before update: {error}")
        })?;
        if !client.is_available_quick() {
            return Ok(());
        }
        client.shutdown_and_wait()
    }

    /// Poll compact status plus Syncthing's typed event feed. This is separate
    /// from the child-process supervisor so native shells can map it into their
    /// own immutable UI state without coupling the engine to a toolkit.
    pub fn start_monitor<Status, Events, Failure>(
        &self,
        folder_id: String,
        hub_device_id: String,
        since: u64,
        on_status: Status,
        on_events: Events,
        on_failure: Failure,
    ) -> SyncthingMonitor
    where
        Status: Fn(SyncStatus) + Send + 'static,
        Events: Fn(Vec<SyncthingEvent>) + Send + 'static,
        Failure: Fn(String) + Send + 'static,
    {
        let client = self.client.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("syncthing-monitor".into())
            .spawn(move || {
                let mut event_id = since;
                while !worker_stop.load(Ordering::Acquire) {
                    match client.sync_status(&folder_id, &hub_device_id) {
                        Ok(status) => on_status(status),
                        Err(error) => {
                            on_failure(error);
                            if worker_stop.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_secs(2));
                            continue;
                        }
                    }
                    match client.poll_events(event_id, 2) {
                        Ok(events) => {
                            if let Some(last) = events.last() {
                                event_id = last.id;
                            }
                            if !events.is_empty() {
                                on_events(events);
                            }
                        }
                        Err(error) => on_failure(error),
                    }
                }
            })
            .expect("spawn Syncthing monitor");
        SyncthingMonitor {
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for SyncthingSupervisor {
    fn drop(&mut self) {
        // The fixed private home/port belongs exclusively to this app. If we
        // attached to a briefly orphaned engine there is no Child handle to
        // reap, but it must still receive shutdown instead of surviving the
        // desktop process indefinitely.
        let _ = self.client.post("/system/shutdown", None);
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(100)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SyncthingMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Obtain the stable local device identity before `/api/pair/start`. The
/// temporary supervisor shuts down immediately when no long-running host is
/// active; the certificate and device ID persist in the private home.
pub fn ensure_local_device_id() -> Result<String, String> {
    let supervisor = SyncthingSupervisor::start()?;
    supervisor.device_id()
}

#[derive(Clone)]
struct SyncthingClient {
    api_key: String,
    agent: ureq::Agent,
}

impl SyncthingClient {
    fn from_home(home: &Path) -> Result<Self, String> {
        let api_key = read_api_key(&home.join("config.xml"))?;
        crate::logs::register_secret(&api_key);
        Ok(Self {
            api_key,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .timeout_read(Duration::from_secs(35))
                .timeout_write(Duration::from_secs(10))
                .build(),
        })
    }

    fn device_id(&self) -> Result<String, String> {
        let status = self.system_status()?;
        validate_device_id(&status.my_id, "local Syncthing device ID")?;
        Ok(status.my_id)
    }

    fn system_status(&self) -> Result<SystemStatus, String> {
        self.get_json("/system/status")
    }

    fn is_available_quick(&self) -> bool {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(500))
            .timeout_read(Duration::from_millis(500))
            .timeout_write(Duration::from_millis(500))
            .build()
            .get(&format!("{API_BASE}/system/status"))
            .set("X-API-Key", &self.api_key)
            .call()
            .is_ok()
    }

    fn shutdown_and_wait(&self) -> Result<(), String> {
        let request_result = self.post("/system/shutdown", None);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !self.is_available_quick() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return match request_result {
                    Ok(()) => Err(
                        "Syncthing did not stop within five seconds; update was not installed."
                            .into(),
                    ),
                    Err(error) => Err(format!("Could not stop Syncthing for update: {error}")),
                };
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn validate_version(&self) -> Result<(), String> {
        let version: SystemVersion = self.get_json("/system/version")?;
        if version.version != REQUIRED_ENGINE_VERSION {
            return Err(format!(
                "Bundled Syncthing version mismatch: expected {REQUIRED_ENGINE_VERSION}, running {}.",
                version.version
            ));
        }
        Ok(())
    }

    fn secure_gui(&self) -> Result<(), String> {
        // The REST API key already authenticates programmatic calls. Reuse it
        // as an unexposed random GUI password so even a same-machine browser
        // cannot open the hidden Syncthing administration interface.
        self.patch_json(
            "/config/gui",
            &json!({
                "user": "backupsynctool",
                "password": self.api_key,
            }),
        )
    }

    fn configure_folder(&self, assignment: &FolderAssignment) -> Result<(), String> {
        let mut config: Value = self.get_json("/config")?;
        let mut default_device: Value = self.get_json("/config/defaults/device")?;
        let mut default_folder: Value = self.get_json("/config/defaults/folder")?;
        let existing_devices = config
            .get("devices")
            .and_then(Value::as_array)
            .ok_or_else(|| "Syncthing returned a config without a devices array.".to_string())?;
        let mut hub_device = existing_devices
            .iter()
            .find(|device| {
                json_string(device, "deviceID")
                    .is_some_and(|id| id.eq_ignore_ascii_case(&assignment.hub_device_id))
            })
            .cloned()
            .unwrap_or_else(|| std::mem::take(&mut default_device));
        configure_hub_device(&mut hub_device, assignment);

        let existing_folders = config
            .get("folders")
            .and_then(Value::as_array)
            .ok_or_else(|| "Syncthing returned a config without a folders array.".to_string())?;
        let mut folder = existing_folders
            .iter()
            .find(|candidate| {
                json_string(candidate, "id").is_some_and(|id| id == assignment.folder_id)
            })
            .cloned()
            .unwrap_or_else(|| std::mem::take(&mut default_folder));
        configure_local_folder(&mut folder, assignment);

        // This engine is private to Backup Sync Tool and has exactly one
        // approved customer assignment. Removing stale devices/folders here
        // ensures a re-pair cannot keep synchronizing a revoked customer.
        config["devices"] = Value::Array(vec![hub_device]);
        config["folders"] = Value::Array(vec![folder]);

        self.put_json("/config", &config)
    }

    fn sync_status(&self, folder_id: &str, hub_device_id: &str) -> Result<SyncStatus, String> {
        validate_folder_id(folder_id)?;
        validate_device_id(hub_device_id, "hub Syncthing device ID")?;
        let database: DatabaseStatus = self.get_json(&format!("/db/status?folder={folder_id}"))?;
        let hub_completion: CompletionStatus = self.get_json(&format!(
            "/db/completion?folder={folder_id}&device={hub_device_id}"
        ))?;
        let connections: Connections = self.get_json("/system/connections")?;
        let connected = connections
            .connections
            .get(hub_device_id)
            .is_some_and(|connection| connection.connected);
        Ok(SyncStatus {
            hub_connected: connected,
            folder_state: database.state,
            local_files: database.local_files,
            global_files: database.global_files,
            need_files: database
                .need_files
                .saturating_add(hub_completion.need_items),
            need_bytes: database
                .need_bytes
                .saturating_add(hub_completion.need_bytes),
        })
    }

    fn scan_folder(&self, folder_id: &str) -> Result<(), String> {
        validate_folder_id(folder_id)?;
        self.post(&format!("/db/scan?folder={folder_id}"), None)
    }

    fn poll_events(&self, since: u64, timeout_seconds: u64) -> Result<Vec<SyncthingEvent>, String> {
        self.get_json(&format!(
            "/events?since={since}&timeout={}",
            timeout_seconds.clamp(1, 30)
        ))
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, String> {
        let response = self
            .agent
            .get(&format!("{API_BASE}{path}"))
            .set("X-API-Key", &self.api_key)
            .call()
            .map_err(rest_error)?;
        let body = response
            .into_string()
            .map_err(|error| format!("Could not read Syncthing response: {error}"))?;
        serde_json::from_str(&body)
            .map_err(|error| format!("Syncthing returned invalid JSON for {path}: {error}"))
    }

    fn put_json(&self, path: &str, value: &Value) -> Result<(), String> {
        let body = serde_json::to_string(value)
            .map_err(|error| format!("Could not encode Syncthing config: {error}"))?;
        self.agent
            .put(&format!("{API_BASE}{path}"))
            .set("X-API-Key", &self.api_key)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(rest_error)?;
        Ok(())
    }

    fn patch_json(&self, path: &str, value: &Value) -> Result<(), String> {
        let body = serde_json::to_string(value)
            .map_err(|error| format!("Could not encode Syncthing config patch: {error}"))?;
        self.agent
            .request("PATCH", &format!("{API_BASE}{path}"))
            .set("X-API-Key", &self.api_key)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(rest_error)?;
        Ok(())
    }

    fn post(&self, path: &str, body: Option<&str>) -> Result<(), String> {
        let request = self
            .agent
            .post(&format!("{API_BASE}{path}"))
            .set("X-API-Key", &self.api_key);
        match body {
            Some(body) => request.send_string(body),
            None => request.call(),
        }
        .map_err(rest_error)?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct SystemStatus {
    #[serde(rename = "myID")]
    my_id: String,
}

#[derive(Deserialize)]
struct SystemVersion {
    version: String,
}

#[derive(Deserialize)]
struct DatabaseStatus {
    #[serde(default)]
    state: String,
    #[serde(rename = "localFiles", default)]
    local_files: u64,
    #[serde(rename = "globalFiles", default)]
    global_files: u64,
    #[serde(rename = "needFiles", default)]
    need_files: u64,
    #[serde(rename = "needBytes", default)]
    need_bytes: u64,
}

#[derive(Deserialize)]
struct CompletionStatus {
    #[serde(rename = "needItems", default)]
    need_items: u64,
    #[serde(rename = "needBytes", default)]
    need_bytes: u64,
}

#[derive(Deserialize)]
struct Connections {
    #[serde(default)]
    connections: std::collections::HashMap<String, DeviceConnection>,
}

#[derive(Deserialize)]
struct DeviceConnection {
    #[serde(default)]
    connected: bool,
}

fn syncthing_command(binary: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg(format!("--home={}", home.display()))
        .arg("--no-browser")
        .arg("--no-restart")
        .arg("--no-upgrade")
        .arg(format!("--gui-address={GUI_ADDRESS}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn forward_engine_output<R: std::io::Read + Send + 'static>(
    reader: Option<R>,
    label: &'static str,
) {
    let Some(reader) = reader else { return };
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            crate::logs::append(&format!("{label}: {line}"));
        }
    });
}

fn read_api_key(path: &Path) -> Result<String, String> {
    let xml = fs::read_to_string(path)
        .map_err(|error| format!("Syncthing API config is not ready: {error}"))?;
    let start = xml
        .find("<apikey>")
        .map(|index| index + "<apikey>".len())
        .ok_or_else(|| "Syncthing config omitted its API key.".to_string())?;
    let end = xml[start..]
        .find("</apikey>")
        .map(|index| start + index)
        .ok_or_else(|| "Syncthing config has an invalid API key element.".to_string())?;
    let key = xml[start..end].trim();
    if key.is_empty() {
        return Err("Syncthing config contains an empty API key.".into());
    }
    Ok(key.to_string())
}

fn rest_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            format!("Syncthing API returned HTTP {status}: {}", body.trim())
        }
        ureq::Error::Transport(error) => format!("Could not reach private Syncthing API: {error}"),
    }
}

fn validate_device_id(value: &str, label: &str) -> Result<(), String> {
    let value = value.trim();
    let valid = value.len() == 63
        && value.split('-').count() == 8
        && value.split('-').all(|group| {
            group.len() == 7
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || matches!(byte, b'2'..=b'7'))
        });
    if !valid {
        return Err(format!("Pairing returned an invalid {label}."));
    }
    Ok(())
}

fn validate_folder_id(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("Pairing returned an invalid Syncthing folder ID.".into());
    }
    Ok(())
}

fn validate_hub_address(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value == "dynamic" {
        return Ok(());
    }
    if !value.chars().any(char::is_whitespace)
        && ["tcp://", "quic://", "relay://"].iter().any(|scheme| {
            value
                .strip_prefix(scheme)
                .is_some_and(|rest| !rest.is_empty())
        })
    {
        return Ok(());
    }
    Err("Pairing returned an invalid Syncthing hub address.".into())
}

fn json_string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn configure_hub_device(device: &mut Value, assignment: &FolderAssignment) {
    device["deviceID"] = Value::String(assignment.hub_device_id.clone());
    device["name"] = Value::String("Backup Hub (CT 105)".into());
    device["addresses"] = json!(assignment.hub_addresses);
    device["compression"] = Value::String("metadata".into());
    device["introducer"] = Value::Bool(false);
    device["autoAcceptFolders"] = Value::Bool(false);
    device["paused"] = Value::Bool(false);
}

fn configure_local_folder(folder: &mut Value, assignment: &FolderAssignment) {
    folder["id"] = Value::String(assignment.folder_id.clone());
    folder["label"] = Value::String(assignment.folder_label.clone());
    folder["filesystemType"] = Value::String("basic".into());
    folder["path"] = Value::String(assignment.path.to_string_lossy().into_owned());
    folder["type"] = Value::String("sendreceive".into());
    folder["devices"] = json!([
        { "deviceID": assignment.local_device_id },
        { "deviceID": assignment.hub_device_id }
    ]);
    folder["rescanIntervalS"] = json!(3600);
    folder["fsWatcherEnabled"] = Value::Bool(true);
    folder["fsWatcherDelayS"] = json!(10);
    folder["paused"] = Value::Bool(false);
    folder["maxConflicts"] = json!(10);
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_ID: &str = "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH";

    #[test]
    fn validates_syncthing_assignment_identifiers() {
        assert!(validate_device_id(DEVICE_ID, "device ID").is_ok());
        assert!(validate_device_id("short", "device ID").is_err());
        for invalid in [
            "aAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
            "0AAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
            "1AAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
            "8AAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
            "9AAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH",
        ] {
            assert!(
                validate_device_id(invalid, "device ID").is_err(),
                "{invalid}"
            );
        }
        assert!(validate_folder_id("xdpt.59655-customer").is_ok());
        assert!(validate_folder_id("../bad").is_err());
        assert!(validate_hub_address("tcp://sync.example:22000").is_ok());
        assert!(validate_hub_address("tcp://").is_err());
        assert!(validate_hub_address("tcp://sync.example:22000\nrelay://bad").is_err());
        assert!(validate_hub_address("https://sync.example").is_err());
    }

    #[test]
    fn extracts_private_api_key() {
        let path = std::env::temp_dir().join(format!(
            "backup-sync-tool-config-{}-{}.xml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "<configuration><gui><apikey>secret-key</apikey></gui></configuration>",
        )
        .unwrap();
        assert_eq!(read_api_key(&path).unwrap(), "secret-key");
        let _ = fs::remove_file(path);
    }
}
