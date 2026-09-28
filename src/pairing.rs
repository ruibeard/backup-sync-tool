use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize)]
pub struct PairStartRequest {
    pub machine_name: String,
    pub windows_user: String,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_install_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_backup_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xd_license_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xd_customer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_customer: Option<String>,
    pub supported_transports: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairStartResponse {
    pub code: String,
    pub approve_url: String,
    /// Laravel APP_URL for this install (optional for older servers).
    #[serde(default)]
    pub control_plane_url: Option<String>,
    pub poll_token: String,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairStatusResponse {
    pub status: String,
    pub device_token: Option<String>,
    #[serde(default)]
    pub device_uuid: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub destination_uuid: Option<String>,
    #[serde(default)]
    pub destination_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingErrorKind {
    Cancelled,
    InvalidRequest,
    Network,
    Http,
    ResponseBody,
    InvalidResponse,
}

/// A pairing failure that preserves enough detail for the UI to distinguish a
/// retryable control-plane problem from a permanent failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingError {
    pub kind: PairingErrorKind,
    pub message: String,
    pub http_status: Option<u16>,
}

impl PairingError {
    fn new(kind: PairingErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            http_status: None,
        }
    }

    fn cancelled() -> Self {
        Self::new(PairingErrorKind::Cancelled, "Pairing cancelled.")
    }

    pub fn is_retryable(&self) -> bool {
        match self.kind {
            PairingErrorKind::Network | PairingErrorKind::ResponseBody => true,
            PairingErrorKind::Http => self
                .http_status
                .is_some_and(|status| status == 408 || status == 429 || status >= 500),
            _ => false,
        }
    }

    pub fn is_transient(&self) -> bool {
        self.is_retryable()
    }
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PairingError {}

/// Pairing start without cancellation (macOS host). Windows uses
/// [`start_pairing_cancellable`].
#[allow(clippy::too_many_arguments)]
pub fn start_pairing_result(
    api_base: &str,
    machine_name: &str,
    windows_user: &str,
    app_version: &str,
    detected_install_path: Option<String>,
    detected_backup_path: Option<String>,
    xd_license_number: Option<String>,
    xd_customer_name: Option<String>,
    suggested_customer: Option<String>,
) -> Result<PairStartResponse, PairingError> {
    let cancel = AtomicBool::new(false);
    start_pairing_cancellable(
        api_base,
        machine_name,
        windows_user,
        app_version,
        detected_install_path,
        detected_backup_path,
        xd_license_number,
        xd_customer_name,
        suggested_customer,
        &cancel,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn start_pairing_cancellable(
    api_base: &str,
    machine_name: &str,
    windows_user: &str,
    app_version: &str,
    detected_install_path: Option<String>,
    detected_backup_path: Option<String>,
    xd_license_number: Option<String>,
    xd_customer_name: Option<String>,
    suggested_customer: Option<String>,
    cancel: &AtomicBool,
) -> Result<PairStartResponse, PairingError> {
    if cancel.load(Ordering::Acquire) {
        return Err(PairingError::cancelled());
    }
    let req = PairStartRequest {
        machine_name: machine_name.to_string(),
        windows_user: windows_user.to_string(),
        app_version: app_version.to_string(),
        detected_install_path,
        detected_backup_path,
        xd_license_number,
        xd_customer_name,
        suggested_customer,
        supported_transports: vec!["chunk_store".to_string()],
    };
    let body = serde_json::to_string(&req).map_err(|err| {
        PairingError::new(
            PairingErrorKind::InvalidRequest,
            format!("Could not encode pairing request: {err}"),
        )
    })?;
    let url = format!("{}/api/pair/start", api_base.trim_end_matches('/'));
    let res = pairing_agent()
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(map_ureq_error)?;
    if cancel.load(Ordering::Acquire) {
        return Err(PairingError::cancelled());
    }
    let body = res.into_string().map_err(|err| {
        PairingError::new(
            PairingErrorKind::ResponseBody,
            format!("Could not read pairing response: {err}"),
        )
    })?;
    let start: PairStartResponse = serde_json::from_str(&body).map_err(|err| {
        PairingError::new(
            PairingErrorKind::InvalidResponse,
            format!("Pairing server returned an invalid start response: {err}"),
        )
    })?;
    validate_start_response(&start)?;
    crate::logs::register_secret(&start.poll_token);
    log_control_plane_mismatch(api_base, &start);
    Ok(start)
}

/// Suggested customer label for a manually selected folder.
/// Uses the same `{hostname}-{folder}` shape on Windows and macOS.
pub fn build_host_folder_hint(machine_name: &str, watch_folder: &str) -> Option<String> {
    let path = Path::new(watch_folder.trim());
    if !path.is_dir() {
        return None;
    }
    let folder_name = path.file_name()?.to_str()?.trim();
    if folder_name.is_empty() {
        return None;
    }

    let machine_slug = slugify_hint(machine_name);
    if machine_slug.is_empty() {
        return None;
    }
    let folder_slug = slugify_hint(folder_name);
    let suggestion = if folder_slug.is_empty() {
        machine_slug
    } else {
        format!("{machine_slug}-{folder_slug}")
    };
    let suggestion = truncate_utf8_bytes(&suggestion, 63)
        .trim_matches('-')
        .to_string();
    (!suggestion.is_empty()).then_some(suggestion)
}

fn slugify_hint(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_dash = false;
    for character in value.trim().nfd() {
        if is_combining_mark(character) {
            continue;
        }
        if character.is_alphanumeric() {
            output.push(character);
            previous_dash = false;
        } else if !previous_dash {
            output.push('-');
            previous_dash = true;
        }
    }
    output.trim_matches('-').to_string()
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn log_control_plane_mismatch(api_base: &str, start: &PairStartResponse) {
    let Some(echoed) = start.control_plane_url.as_deref() else {
        return;
    };
    let configured = api_base.trim_end_matches('/');
    let echoed = echoed.trim_end_matches('/');
    if !echoed.eq_ignore_ascii_case(configured) {
        crate::logs::append(&format!(
            "control_plane_url mismatch: configured={configured} echoed={echoed}"
        ));
    }
}

pub fn poll_pairing_result(
    api_base: &str,
    poll_token: &str,
) -> Result<PairStatusResponse, PairingError> {
    let cancel = AtomicBool::new(false);
    poll_pairing_cancellable(api_base, poll_token, &cancel)
}

pub fn poll_pairing_cancellable(
    api_base: &str,
    poll_token: &str,
    cancel: &AtomicBool,
) -> Result<PairStatusResponse, PairingError> {
    if cancel.load(Ordering::Acquire) {
        return Err(PairingError::cancelled());
    }
    if poll_token.trim().is_empty() {
        return Err(PairingError::new(
            PairingErrorKind::InvalidRequest,
            "Pairing poll token is empty.",
        ));
    }
    let url = format!(
        "{}/api/pair/status/{}",
        api_base.trim_end_matches('/'),
        poll_token
    );
    let res = pairing_agent().get(&url).call().map_err(map_ureq_error)?;
    if cancel.load(Ordering::Acquire) {
        return Err(PairingError::cancelled());
    }
    let body = res.into_string().map_err(|err| {
        PairingError::new(
            PairingErrorKind::ResponseBody,
            format!("Could not read pairing status: {err}"),
        )
    })?;
    let status: PairStatusResponse = serde_json::from_str(&body).map_err(|err| {
        PairingError::new(
            PairingErrorKind::InvalidResponse,
            format!("Pairing server returned an invalid status response: {err}"),
        )
    })?;
    register_approval_secrets(&status);
    Ok(status)
}

fn pairing_agent() -> ureq::Agent {
    // Blocking HTTP cannot be interrupted mid-syscall. Short request timeouts
    // bound cancellation latency while retaining the required blocking stack.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build()
}

fn register_approval_secrets(status: &PairStatusResponse) {
    if let Some(token) = status.device_token.as_deref() {
        crate::logs::register_secret(token);
    }
}

fn map_ureq_error(err: ureq::Error) -> PairingError {
    match err {
        ureq::Error::Status(status, _) => PairingError {
            kind: PairingErrorKind::Http,
            message: format!("Pairing server returned HTTP {status}."),
            http_status: Some(status),
        },
        ureq::Error::Transport(err) => PairingError::new(
            PairingErrorKind::Network,
            format!("Could not reach pairing server: {err}"),
        ),
    }
}

fn validate_start_response(start: &PairStartResponse) -> Result<(), PairingError> {
    for (value, label) in [
        (&start.code, "pairing code"),
        (&start.approve_url, "approval URL"),
        (&start.poll_token, "poll token"),
    ] {
        if value.trim().is_empty() {
            return Err(PairingError::new(
                PairingErrorKind::InvalidResponse,
                format!("Pairing server omitted {label}."),
            ));
        }
    }
    if start.poll_interval_ms == 0 {
        return Err(PairingError::new(
            PairingErrorKind::InvalidResponse,
            "Pairing server returned an invalid poll interval.",
        ));
    }
    Ok(())
}

pub fn is_chunk_store_approval(status: &PairStatusResponse) -> bool {
    status
        .transport
        .as_deref()
        .is_some_and(|transport| transport.eq_ignore_ascii_case("chunk_store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_start_serializes_chunk_store_transport() {
        let req = PairStartRequest {
            machine_name: "PC".into(),
            windows_user: "u".into(),
            app_version: "2026.2.0".into(),
            detected_install_path: Some(r"C:\XDSoftware".into()),
            detected_backup_path: Some(r"C:\XDSoftware\backups".into()),
            xd_license_number: Some("XDPT.1".into()),
            xd_customer_name: Some("Customer".into()),
            suggested_customer: Some("XDPT.1-Customer".into()),
            supported_transports: vec!["chunk_store".into()],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json["supported_transports"],
            serde_json::json!(["chunk_store"])
        );
        assert_eq!(json["xd_license_number"], "XDPT.1");
    }

    #[test]
    fn host_folder_hint_includes_machine_and_selected_folder() {
        let folder = std::env::temp_dir().join("manual backup folder");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(
            build_host_folder_hint("Rui's Mac.local", folder.to_str().unwrap()).as_deref(),
            Some("Rui-s-Mac-local-manual-backup-folder")
        );
    }

    #[test]
    fn host_folder_hint_fits_backend_destination_limit() {
        let folder = std::env::temp_dir().join("a-very-long-manual-backup-folder-name");
        std::fs::create_dir_all(&folder).unwrap();
        let hint = build_host_folder_hint(
            "an-extremely-long-machine-name-that-would-overflow-the-limit",
            folder.to_str().unwrap(),
        )
        .unwrap();
        assert!(hint.len() <= 63);
        assert!(!hint.ends_with('-'));
    }

    #[test]
    fn chunk_store_approval_requires_transport_field() {
        let approved = PairStatusResponse {
            status: "approved".into(),
            device_token: Some("t".into()),
            device_uuid: Some("device-uuid".into()),
            transport: Some("chunk_store".into()),
            destination_uuid: Some("dest".into()),
            destination_label: Some("Customer 1".into()),
        };
        assert!(is_chunk_store_approval(&approved));

        let not_ok = PairStatusResponse {
            transport: None,
            ..approved
        };
        assert!(!is_chunk_store_approval(&not_ok));
    }

    #[test]
    fn pair_start_response_control_plane_url_optional() {
        let with_url = r#"{
            "code": "ABC123",
            "approve_url": "https://backup.rui.cam/pair/ABC123",
            "control_plane_url": "https://backup.rui.cam",
            "poll_token": "tok",
            "poll_interval_ms": 2000
        }"#;
        let parsed: PairStartResponse = serde_json::from_str(with_url).unwrap();
        assert_eq!(
            parsed.control_plane_url.as_deref(),
            Some("https://backup.rui.cam")
        );

        let without_url = r#"{
            "code": "ABC123",
            "approve_url": "https://backup.rui.cam/pair/ABC123",
            "poll_token": "tok",
            "poll_interval_ms": 2000
        }"#;
        let parsed: PairStartResponse = serde_json::from_str(without_url).unwrap();
        assert_eq!(parsed.control_plane_url, None);
    }

    #[test]
    fn cancellation_is_typed_before_network_io() {
        let cancel = AtomicBool::new(true);
        let err =
            poll_pairing_cancellable("https://example.invalid", "token", &cancel).unwrap_err();
        assert_eq!(err.kind, PairingErrorKind::Cancelled);
        assert!(!err.is_transient());
    }

    #[test]
    fn start_response_requires_polling_fields() {
        let response = PairStartResponse {
            code: String::new(),
            approve_url: "https://example.test/approve".into(),
            control_plane_url: None,
            poll_token: "token".into(),
            poll_interval_ms: 2_000,
        };
        assert_eq!(
            validate_start_response(&response).unwrap_err().kind,
            PairingErrorKind::InvalidResponse
        );
    }
}
