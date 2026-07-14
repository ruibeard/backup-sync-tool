//! Persistent S3 multipart upload helpers (Phase 2).
//!
//! Resume state lives under [`crate::paths::multipart_state_dir`]
//! (outside the watched folder). Filename is SHA-256 of the storage identity.

use crate::transport::TransportError;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Bumped when resume schema changes (sha256 parts, mtime_ns, Verified phase).
pub const STATE_VERSION: u32 = 2;
pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;
pub const MAX_PARTS: u32 = 10_000;
/// Hard cap on buffered UploadPart size (and on configured growth).
pub const MAX_BUFFERED_PART_SIZE: u64 = 64 * 1024 * 1024;
pub const MAX_MULTIPART_OBJECT_SIZE: u64 = MAX_BUFFERED_PART_SIZE * MAX_PARTS as u64;
pub const LIST_PARTS_PAGE_CAP: u32 = 20;
pub const META_UPLOAD_TOKEN: &str = "x-amz-meta-backup-upload-token";
pub const RETRY_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultipartPhase {
    Uploading,
    Completing,
    /// Object verified (size+token). Kept until source identity changes.
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedPart {
    pub number: u32,
    pub etag: String,
    #[serde(default)]
    pub size: u64,
    /// Hex SHA-256 of the local part bytes at upload time.
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartState {
    pub version: u32,
    pub endpoint: String,
    pub bucket: String,
    pub object_key: String,
    pub local_path: String,
    pub local_size: u64,
    /// Subsecond source identity (nanoseconds since Unix epoch).
    pub local_mtime_ns: u64,
    pub part_size: u64,
    pub upload_id: String,
    pub client_upload_token: String,
    pub completed_parts: Vec<CompletedPart>,
    pub phase: MultipartPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPart {
    pub number: u32,
    pub etag: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPartsPage {
    pub parts: Vec<ServerPart>,
    pub is_truncated: bool,
    pub next_part_number_marker: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileResult {
    /// Local parts that also match server ETag+size (never server-only).
    pub parts: Vec<CompletedPart>,
    /// Part numbers still needing UploadPart.
    pub missing: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostCompleteDecision {
    Success,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedReceiptDecision {
    /// Unchanged source + HEAD size/token match — return success without reupload.
    ReuseSuccess,
    /// Clear receipt (no Abort) and start a new MPU.
    ClearAndRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectVerifyHead {
    pub size: u64,
    pub upload_token: Option<String>,
}

/// SHA-256 hex of `endpoint\0bucket\0object_key` (storage identity).
pub fn storage_identity(endpoint: &str, bucket: &str, object_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(endpoint.trim().trim_end_matches('/').as_bytes());
    hasher.update([0]);
    hasher.update(bucket.trim().as_bytes());
    hasher.update([0]);
    hasher.update(object_key.trim_start_matches('/').as_bytes());
    hex::encode(hasher.finalize())
}

pub fn multipart_state_dir() -> PathBuf {
    crate::paths::multipart_state_dir()
}

pub fn state_path_for_identity(dir: &Path, identity: &str) -> PathBuf {
    dir.join(format!("{identity}.json"))
}

/// Overflow-safe ceiling division.
pub fn ceil_div(numer: u64, denom: u64) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    Some(numer / denom + u64::from(numer % denom != 0))
}

/// Choose part size: configured 16–64 MiB, never buffer above 64 MiB.
/// Reject objects above `64 MiB * 10_000` instead of growing toward 5 GiB.
pub fn choose_part_size(file_size: u64, configured_mib: u64) -> Result<u64, TransportError> {
    if file_size > MAX_MULTIPART_OBJECT_SIZE {
        return Err(TransportError::TooLarge {
            size: file_size,
            limit: MAX_MULTIPART_OBJECT_SIZE,
        });
    }
    if file_size == 0 {
        return Ok(MIN_PART_SIZE.min(MAX_BUFFERED_PART_SIZE));
    }
    let configured = (configured_mib.clamp(16, 64) * 1024 * 1024)
        .max(MIN_PART_SIZE)
        .min(MAX_BUFFERED_PART_SIZE);
    let mut part_size = configured;
    let parts = ceil_div(file_size, part_size).ok_or_else(|| {
        TransportError::Other("Invalid part size while choosing multipart layout".into())
    })?;
    if parts > u64::from(MAX_PARTS) {
        part_size = MAX_BUFFERED_PART_SIZE;
        let parts_at_cap = ceil_div(file_size, part_size).ok_or_else(|| {
            TransportError::Other("Invalid part size while choosing multipart layout".into())
        })?;
        if parts_at_cap > u64::from(MAX_PARTS) {
            return Err(TransportError::TooLarge {
                size: file_size,
                limit: MAX_MULTIPART_OBJECT_SIZE,
            });
        }
    }
    Ok(part_size)
}

pub fn part_count(file_size: u64, part_size: u64) -> u32 {
    if file_size == 0 || part_size == 0 {
        return 1;
    }
    ceil_div(file_size, part_size)
        .unwrap_or(1)
        .min(u64::from(MAX_PARTS)) as u32
}

pub fn part_offset(part_number: u32, part_size: u64) -> u64 {
    u64::from(part_number.saturating_sub(1)).saturating_mul(part_size)
}

pub fn expected_part_size(part_number: u32, file_size: u64, part_size: u64) -> u64 {
    let total = part_count(file_size, part_size);
    if part_number == 0 || part_number > total {
        return 0;
    }
    let offset = part_offset(part_number, part_size);
    if offset >= file_size {
        return 0;
    }
    let remaining = file_size - offset;
    remaining.min(part_size)
}

pub fn source_changed(state: &MultipartState, size: u64, mtime_ns: u64) -> bool {
    state.local_size != size || state.local_mtime_ns != mtime_ns
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Retain only local parts that still match server ETag+size (+ expected size).
/// Never adopt server-only parts (prevents hybrid resumes).
pub fn reconcile_parts(
    local: &[CompletedPart],
    server: &[ServerPart],
    file_size: u64,
    part_size: u64,
) -> ReconcileResult {
    let total = part_count(file_size, part_size);
    let server_by_number: HashMap<u32, &ServerPart> =
        server.iter().map(|p| (p.number, p)).collect();

    let mut retained = Vec::new();
    for lp in local {
        if lp.number == 0 || lp.number > total || lp.sha256.is_empty() {
            continue;
        }
        let expected = expected_part_size(lp.number, file_size, part_size);
        if lp.size != expected {
            continue;
        }
        let Some(sp) = server_by_number.get(&lp.number) else {
            continue;
        };
        if sp.size != lp.size {
            continue;
        }
        if normalize_etag(&sp.etag) != normalize_etag(&lp.etag) {
            continue;
        }
        retained.push(CompletedPart {
            number: lp.number,
            etag: normalize_etag(&lp.etag),
            size: lp.size,
            sha256: lp.sha256.clone(),
        });
    }
    retained.sort_by_key(|p| p.number);
    let kept: std::collections::HashSet<u32> = retained.iter().map(|p| p.number).collect();
    let missing: Vec<u32> = (1..=total).filter(|n| !kept.contains(n)).collect();
    ReconcileResult {
        parts: retained,
        missing,
    }
}

/// Decide whether a retained part's local bytes still match the stored digest.
pub fn retained_part_digest_ok(
    part: &CompletedPart,
    file_size: u64,
    part_size: u64,
    current_digest: &str,
) -> bool {
    if part.sha256.is_empty() || current_digest != part.sha256 {
        return false;
    }
    let expected = expected_part_size(part.number, file_size, part_size);
    part.size == expected
}

pub fn decide_after_lost_complete(
    head: Option<&ObjectVerifyHead>,
    expected_size: u64,
    expected_token: &str,
) -> LostCompleteDecision {
    match head {
        Some(h)
            if h.size == expected_size
                && h.upload_token.as_deref() == Some(expected_token)
                && !expected_token.is_empty() =>
        {
            LostCompleteDecision::Success
        }
        _ => LostCompleteDecision::Restart,
    }
}

pub fn decide_verified_receipt(
    source_changed: bool,
    head: Option<&ObjectVerifyHead>,
    expected_size: u64,
    expected_token: &str,
) -> VerifiedReceiptDecision {
    if source_changed {
        return VerifiedReceiptDecision::ClearAndRestart;
    }
    match decide_after_lost_complete(head, expected_size, expected_token) {
        LostCompleteDecision::Success => VerifiedReceiptDecision::ReuseSuccess,
        LostCompleteDecision::Restart => VerifiedReceiptDecision::ClearAndRestart,
    }
}

pub fn normalize_etag(etag: &str) -> String {
    let t = etag.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        t.to_string()
    } else if t.is_empty() {
        t.to_string()
    } else {
        format!("\"{t}\"")
    }
}

pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn build_complete_xml(parts: &[CompletedPart]) -> String {
    let mut sorted = parts.to_vec();
    sorted.sort_by_key(|p| p.number);
    let mut xml = String::from("<CompleteMultipartUpload>");
    for part in &sorted {
        xml.push_str("<Part><PartNumber>");
        xml.push_str(&part.number.to_string());
        xml.push_str("</PartNumber><ETag>");
        xml.push_str(&escape_xml(&normalize_etag(&part.etag)));
        xml.push_str("</ETag></Part>");
    }
    xml.push_str("</CompleteMultipartUpload>");
    xml
}

pub fn new_client_upload_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(b"backupsynctool-upload-token-v1");
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hex::encode(hasher.finalize())[..32].to_string()
}

pub fn ensure_state_dir(dir: &Path) -> Result<(), TransportError> {
    crate::paths::ensure_dir(dir).map_err(|e| TransportError::Other(e.to_string()))
}

pub fn load_state(path: &Path) -> Result<Option<MultipartState>, TransportError> {
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(path).map_err(|e| TransportError::Other(e.to_string()))?;
    let state: MultipartState = serde_json::from_str(&data)
        .map_err(|e| TransportError::Other(format!("multipart state JSON: {e}")))?;
    if state.version != STATE_VERSION {
        return Ok(None);
    }
    Ok(Some(state))
}

pub fn save_state_atomic(path: &Path, state: &MultipartState) -> Result<(), TransportError> {
    if let Some(parent) = path.parent() {
        ensure_state_dir(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| TransportError::Other(format!("multipart state serialize: {e}")))?;
    {
        let mut f = File::create(&tmp).map_err(|e| TransportError::Other(e.to_string()))?;
        f.write_all(json.as_bytes())
            .map_err(|e| TransportError::Other(e.to_string()))?;
        f.sync_all()
            .map_err(|e| TransportError::Other(e.to_string()))?;
    }
    replace_file(&tmp, path).map_err(|e| TransportError::Other(e.to_string()))
}

pub fn delete_state(path: &Path) -> Result<(), TransportError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(TransportError::Other(e.to_string())),
    }
}

/// Advance ListParts pagination: truncated pages need a strictly increasing marker.
/// Returns `Ok(None)` when listing is complete.
pub fn next_list_parts_marker(
    previous_marker: Option<u32>,
    page: &ListPartsPage,
    pages_seen: u32,
) -> Result<Option<u32>, TransportError> {
    if !page.is_truncated {
        return Ok(None);
    }
    if pages_seen >= LIST_PARTS_PAGE_CAP {
        return Err(TransportError::Other(
            "ListParts pagination exceeded page cap".into(),
        ));
    }
    let next = match page.next_part_number_marker {
        Some(m) if m > 0 => m,
        _ => {
            return Err(TransportError::Other(
                "ListParts truncated without NextPartNumberMarker".into(),
            ))
        }
    };
    if let Some(prev) = previous_marker {
        if next <= prev {
            return Err(TransportError::Other(
                "ListParts pagination marker did not strictly increase".into(),
            ));
        }
    }
    Ok(Some(next))
}

// --- in-process per-identity upload lock ------------------------------------

fn active_identity_set() -> &'static (Mutex<HashSet<String>>, Condvar) {
    static ACTIVE: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();
    ACTIVE.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

/// RAII guard serializing multipart uploads for one storage identity in-process.
pub struct IdentityUploadGuard {
    identity: String,
}

impl IdentityUploadGuard {
    pub fn acquire(identity: &str) -> Self {
        let identity = identity.to_string();
        let (active, changed) = active_identity_set();
        let mut guard = active.lock().unwrap_or_else(|error| error.into_inner());
        while guard.contains(&identity) {
            guard = changed
                .wait(guard)
                .unwrap_or_else(|error| error.into_inner());
        }
        guard.insert(identity.clone());
        Self { identity }
    }

    pub fn try_acquire(identity: &str) -> Option<Self> {
        let identity = identity.to_string();
        let (active, _) = active_identity_set();
        let mut guard = active.lock().ok()?;
        if !guard.insert(identity.clone()) {
            return None;
        }
        Some(Self { identity })
    }
}

impl Drop for IdentityUploadGuard {
    fn drop(&mut self) {
        let (active, changed) = active_identity_set();
        let mut guard = active.lock().unwrap_or_else(|error| error.into_inner());
        guard.remove(&self.identity);
        changed.notify_all();
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| std::io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    let _ = fs::remove_file(destination);
    fs::rename(source, destination)
}

pub fn parse_upload_id(xml: &str) -> Result<String, TransportError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut in_upload_id = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == "UploadId" => {
                in_upload_id = true;
            }
            Ok(Event::Text(e)) if in_upload_id => {
                let id = e.unescape().map(|c| c.into_owned()).unwrap_or_default();
                if id.is_empty() {
                    return Err(TransportError::Other(
                        "Empty UploadId in CreateMultipartUpload".into(),
                    ));
                }
                return Ok(id);
            }
            Ok(Event::End(e)) if local_name(e.name().as_ref()) == "UploadId" => {
                in_upload_id = false;
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(TransportError::Other(format!(
                    "CreateMultipartUpload XML parse error: {err}"
                )))
            }
            _ => {}
        }
    }
    Err(TransportError::Other(
        "UploadId missing in CreateMultipartUpload response".into(),
    ))
}

pub fn parse_list_parts(xml: &str) -> Result<ListPartsPage, TransportError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut page = ListPartsPage {
        parts: Vec::new(),
        is_truncated: false,
        next_part_number_marker: None,
    };
    let mut current: Option<ServerPart> = None;
    let mut text_target = String::new();
    let mut in_part = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = local_name(e.name().as_ref());
                match tag.as_str() {
                    "Part" => {
                        in_part = true;
                        current = Some(ServerPart {
                            number: 0,
                            etag: String::new(),
                            size: 0,
                        });
                    }
                    "PartNumber" | "ETag" | "Size" | "IsTruncated" | "NextPartNumberMarker" => {
                        text_target = tag;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().map(|c| c.into_owned()).unwrap_or_default();
                match text_target.as_str() {
                    "PartNumber" if in_part => {
                        if let Some(p) = current.as_mut() {
                            p.number = text.parse().unwrap_or(0);
                        }
                    }
                    "ETag" if in_part => {
                        if let Some(p) = current.as_mut() {
                            p.etag = text;
                        }
                    }
                    "Size" if in_part => {
                        if let Some(p) = current.as_mut() {
                            p.size = text.parse().unwrap_or(0);
                        }
                    }
                    "IsTruncated" => {
                        page.is_truncated = text.eq_ignore_ascii_case("true");
                    }
                    "NextPartNumberMarker" => {
                        page.next_part_number_marker = text.parse().ok();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(e.name().as_ref());
                if tag == "Part" {
                    if let Some(p) = current.take() {
                        if p.number > 0 {
                            page.parts.push(p);
                        }
                    }
                    in_part = false;
                }
                if tag == text_target {
                    text_target.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(TransportError::Other(format!(
                    "ListParts XML parse error: {err}"
                )))
            }
            _ => {}
        }
    }
    Ok(page)
}

pub fn complete_response_error(xml: &str) -> Option<String> {
    let trimmed = xml.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("CompleteMultipartUploadResult") && !trimmed.contains("<Error") {
        return None;
    }
    extract_error_code(trimmed)
}

pub fn extract_error_code(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut in_code = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == "Code" => in_code = true,
            Ok(Event::Text(e)) if in_code => {
                return Some(e.unescape().map(|c| c.into_owned()).unwrap_or_default());
            }
            Ok(Event::End(e)) if local_name(e.name().as_ref()) == "Code" => in_code = false,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

pub fn is_no_such_upload(err: &TransportError) -> bool {
    match err {
        TransportError::Http(_, action) => action.contains("NoSuchUpload"),
        TransportError::Other(msg) => msg.contains("NoSuchUpload"),
        _ => false,
    }
}

pub fn is_transient(err: &TransportError) -> bool {
    if err.is_auth_failed() || err.is_source_changed() {
        return false;
    }
    match err {
        TransportError::Http(status, action) => {
            if action.contains("XML") || action.contains("parse") {
                return false;
            }
            matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
        }
        TransportError::Other(msg) => {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("xml") || lower.contains("parse") || lower.contains("changed") {
                return false;
            }
            lower.contains("timed out")
                || lower.contains("timeout")
                || lower.contains("connection")
                || lower.contains("reset")
                || lower.contains("broken pipe")
                || lower.contains("temporarily")
        }
        TransportError::NotFound
        | TransportError::Cancelled
        | TransportError::TooLarge { .. }
        | TransportError::AuthFailed(_)
        | TransportError::SourceChanged => false,
    }
}

pub fn sleep_backoff(attempt: u32) {
    let secs = 1u64 << attempt.min(2);
    std::thread::sleep(Duration::from_secs(secs));
}

pub fn read_part_buffer(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, TransportError> {
    if len > MAX_BUFFERED_PART_SIZE {
        return Err(TransportError::Other(format!(
            "Refusing to buffer part of {len} bytes (cap {MAX_BUFFERED_PART_SIZE})"
        )));
    }
    let mut file = File::open(path).map_err(|e| TransportError::Other(e.to_string()))?;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|e| TransportError::Other(e.to_string()))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)
        .map_err(|e| TransportError::Other(e.to_string()))?;
    Ok(buf)
}

fn local_name(name: &[u8]) -> String {
    let name = std::str::from_utf8(name).unwrap_or_default();
    name.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_part(number: u32, etag: &str, size: u64, sha: &str) -> CompletedPart {
        CompletedPart {
            number,
            etag: etag.into(),
            size,
            sha256: sha.into(),
        }
    }

    #[test]
    fn part_math_boundaries() {
        let part = 32 * 1024 * 1024;
        assert_eq!(part_count(part, part), 1);
        assert_eq!(part_count(part + 1, part), 2);
        assert_eq!(expected_part_size(1, part + 1, part), part);
        assert_eq!(expected_part_size(2, part + 1, part), 1);
        assert_eq!(part_offset(3, part), 2 * part);
    }

    #[test]
    fn ceil_div_overflow_safe() {
        assert_eq!(ceil_div(0, 10), Some(0));
        assert_eq!(ceil_div(10, 10), Some(1));
        assert_eq!(ceil_div(11, 10), Some(2));
        assert_eq!(ceil_div(u64::MAX, 2), Some(1u64 << 63));
        assert_eq!(ceil_div(1, 0), None);
    }

    #[test]
    fn part_math_caps_at_64_mib_and_rejects_beyond() {
        let just_ok = MAX_MULTIPART_OBJECT_SIZE;
        let size = choose_part_size(just_ok, 32).unwrap();
        assert_eq!(size, MAX_BUFFERED_PART_SIZE);
        assert!(part_count(just_ok, size) <= MAX_PARTS);

        let too_big = MAX_MULTIPART_OBJECT_SIZE + 1;
        assert!(matches!(
            choose_part_size(too_big, 64),
            Err(TransportError::TooLarge { limit, .. }) if limit == MAX_MULTIPART_OBJECT_SIZE
        ));
    }

    #[test]
    fn choose_part_size_never_exceeds_64_mib_buffer() {
        // Needs more than 10k parts at 32 MiB → bump to 64 MiB cap, not beyond.
        let needs_grow = 32 * 1024 * 1024 * 10_000 + 1;
        let size = choose_part_size(needs_grow, 32).unwrap();
        assert!(size <= MAX_BUFFERED_PART_SIZE);
        assert_eq!(size, MAX_BUFFERED_PART_SIZE);
    }

    #[test]
    fn storage_identity_stable_and_distinct() {
        let a = storage_identity("https://s3.rui.cam", "bucket", "a/b.zip");
        let b = storage_identity("https://s3.rui.cam/", "bucket", "/a/b.zip");
        let c = storage_identity("https://s3.rui.cam", "bucket", "a/c.zip");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn state_roundtrip_atomic_v2() {
        let dir = std::env::temp_dir().join(format!("bst-mp-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_state_dir(&dir).unwrap();
        let path = state_path_for_identity(&dir, "abc123");
        let state = MultipartState {
            version: STATE_VERSION,
            endpoint: "https://s3.rui.cam".into(),
            bucket: "device".into(),
            object_key: "file.zip".into(),
            local_path: r"C:\backups\file.zip".into(),
            local_size: 100,
            local_mtime_ns: 42_000_000_123,
            part_size: 32 * 1024 * 1024,
            upload_id: "upload-1".into(),
            client_upload_token: "token-1".into(),
            completed_parts: vec![sample_part(1, "\"etag\"", 100, "deadbeef")],
            phase: MultipartPhase::Verified,
        };
        save_state_atomic(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().unwrap();
        assert_eq!(loaded, state);
        delete_state(&path).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_never_adopts_server_only_parts() {
        let part_size = 10u64;
        let file_size = 25u64;
        let server = vec![
            ServerPart {
                number: 1,
                etag: "aaa".into(),
                size: 10,
            },
            ServerPart {
                number: 2,
                etag: "bbb".into(),
                size: 10,
            },
            ServerPart {
                number: 3,
                etag: "ccc".into(),
                size: 5,
            },
        ];
        let local = vec![sample_part(1, "\"aaa\"", 10, "sha-1")];
        let result = reconcile_parts(&local, &server, file_size, part_size);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.parts[0].number, 1);
        assert_eq!(result.parts[0].sha256, "sha-1");
        assert_eq!(result.missing, vec![2, 3]);
    }

    #[test]
    fn reconcile_drops_etag_or_size_mismatch() {
        let local = vec![
            sample_part(1, "\"aaa\"", 10, "sha-1"),
            sample_part(2, "\"old\"", 10, "sha-2"),
        ];
        let server = vec![
            ServerPart {
                number: 1,
                etag: "aaa".into(),
                size: 10,
            },
            ServerPart {
                number: 2,
                etag: "new".into(),
                size: 10,
            },
        ];
        let result = reconcile_parts(&local, &server, 20, 10);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.parts[0].number, 1);
        assert_eq!(result.missing, vec![2]);
    }

    #[test]
    fn same_size_content_change_fails_digest_check() {
        let part = sample_part(1, "\"e\"", 10, &sha256_hex(b"abcdefghij"));
        assert!(retained_part_digest_ok(
            &part,
            10,
            10,
            &sha256_hex(b"abcdefghij")
        ));
        assert!(!retained_part_digest_ok(
            &part,
            10,
            10,
            &sha256_hex(b"ABCDEFGHIJ")
        ));
    }

    #[test]
    fn source_change_uses_nanoseconds() {
        let state = MultipartState {
            version: STATE_VERSION,
            endpoint: String::new(),
            bucket: String::new(),
            object_key: String::new(),
            local_path: String::new(),
            local_size: 10,
            local_mtime_ns: 5_000_000_000,
            part_size: 5,
            upload_id: String::new(),
            client_upload_token: String::new(),
            completed_parts: Vec::new(),
            phase: MultipartPhase::Uploading,
        };
        assert!(source_changed(&state, 10, 5_000_000_001)); // same second, different ns
        assert!(!source_changed(&state, 10, 5_000_000_000));
    }

    #[test]
    fn verified_receipt_decision() {
        let head = ObjectVerifyHead {
            size: 100,
            upload_token: Some("tok".into()),
        };
        assert_eq!(
            decide_verified_receipt(false, Some(&head), 100, "tok"),
            VerifiedReceiptDecision::ReuseSuccess
        );
        assert_eq!(
            decide_verified_receipt(true, Some(&head), 100, "tok"),
            VerifiedReceiptDecision::ClearAndRestart
        );
        assert_eq!(
            decide_verified_receipt(false, None, 100, "tok"),
            VerifiedReceiptDecision::ClearAndRestart
        );
    }

    #[test]
    fn lost_complete_requires_token_not_size_alone() {
        let head = ObjectVerifyHead {
            size: 100,
            upload_token: Some("tok".into()),
        };
        assert_eq!(
            decide_after_lost_complete(Some(&head), 100, "tok"),
            LostCompleteDecision::Success
        );
        assert_eq!(
            decide_after_lost_complete(
                Some(&ObjectVerifyHead {
                    size: 100,
                    upload_token: None,
                }),
                100,
                "tok"
            ),
            LostCompleteDecision::Restart
        );
    }

    #[test]
    fn list_parts_pagination_requires_progress() {
        let page = ListPartsPage {
            parts: vec![],
            is_truncated: true,
            next_part_number_marker: Some(10),
        };
        assert_eq!(next_list_parts_marker(None, &page, 1).unwrap(), Some(10));
        assert_eq!(next_list_parts_marker(Some(5), &page, 2).unwrap(), Some(10));
        assert!(next_list_parts_marker(Some(10), &page, 3).is_err());
        assert!(next_list_parts_marker(Some(5), &page, LIST_PARTS_PAGE_CAP).is_err());

        let done = ListPartsPage {
            parts: vec![],
            is_truncated: false,
            next_part_number_marker: None,
        };
        assert_eq!(next_list_parts_marker(Some(10), &done, 2).unwrap(), None);

        let bad = ListPartsPage {
            parts: vec![],
            is_truncated: true,
            next_part_number_marker: None,
        };
        assert!(next_list_parts_marker(None, &bad, 1).is_err());
    }

    #[test]
    fn identity_lock_raii_exclusive() {
        let id = format!("lock-test-{}", std::process::id());
        let g1 = IdentityUploadGuard::acquire(&id);
        assert!(IdentityUploadGuard::try_acquire(&id).is_none());
        drop(g1);
        assert!(IdentityUploadGuard::try_acquire(&id).is_some());
    }

    #[test]
    fn etag_and_xml_escaping() {
        assert_eq!(normalize_etag("abc"), "\"abc\"");
        let xml = build_complete_xml(&[sample_part(1, "a&", 1, "x")]);
        assert!(xml.contains("<ETag>&quot;a&amp;&quot;</ETag>"));
    }

    #[test]
    fn list_parts_namespace_xml() {
        let xml = r#"<?xml version="1.0"?>
<ListPartsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <IsTruncated>true</IsTruncated>
  <NextPartNumberMarker>1</NextPartNumberMarker>
  <Part>
    <PartNumber>1</PartNumber>
    <ETag>&quot;etag-1&quot;</ETag>
    <Size>10</Size>
  </Part>
</ListPartsResult>"#;
        let page = parse_list_parts(xml).unwrap();
        assert!(page.is_truncated);
        assert_eq!(page.next_part_number_marker, Some(1));
    }

    #[test]
    fn query_signing_uploads_and_parts_sort() {
        fn canon(params: &[(&str, &str)]) -> String {
            let mut encoded: Vec<(String, String)> = params
                .iter()
                .map(|(k, v)| {
                    (
                        crate::transport::s3::uri_encode(k),
                        crate::transport::s3::uri_encode(v),
                    )
                })
                .collect();
            encoded.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            encoded
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        }
        assert_eq!(canon(&[("uploads", "")]), "uploads=");
        assert_eq!(
            canon(&[("uploadId", "abc/def"), ("partNumber", "2")]),
            "partNumber=2&uploadId=abc%2Fdef"
        );
    }

    #[test]
    fn transient_retry_classification() {
        assert!(is_transient(&TransportError::Http(
            503,
            "S3:SlowDown".into()
        )));
        assert!(!is_transient(&TransportError::SourceChanged));
        assert!(is_no_such_upload(&TransportError::Http(
            404,
            "S3:NoSuchUpload".into()
        )));
    }
}
