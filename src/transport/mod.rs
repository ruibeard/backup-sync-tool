// transport/mod.rs — object-safe backup storage backends (S3)

mod download;
mod s3;

use crate::config::{self, Config};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

pub use s3::S3Transport;

#[derive(Debug, Clone)]
pub enum TransportError {
    AuthFailed(String),
    Cancelled,
    Http(u16, String),
    NotFound,
    TooLarge {
        size: u64,
        limit: u64,
    },
    /// Multipart finished and verified, but the local source changed afterward —
    /// callers must not update the local manifest.
    SourceChanged,
    Other(String),
}

impl TransportError {
    pub fn is_auth_failed(&self) -> bool {
        matches!(self, TransportError::AuthFailed(_))
    }

    pub fn is_source_changed(&self) -> bool {
        matches!(self, TransportError::SourceChanged)
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, TransportError::Cancelled)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::AuthFailed(message) => write!(f, "{message}"),
            TransportError::Cancelled => f.write_str("Transfer cancelled"),
            TransportError::Http(status, action) => write!(f, "{action} returned HTTP {status}"),
            TransportError::NotFound => write!(f, "Object not found"),
            TransportError::TooLarge { size, limit } => write!(
                f,
                "File size {size} exceeds multipart capacity (limit {limit})"
            ),
            TransportError::SourceChanged => {
                write!(f, "Source file changed after multipart verification")
            }
            TransportError::Other(message) => f.write_str(message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub transferred: u64,
    pub total: u64,
}

pub type ProgressFn = Arc<dyn Fn(TransferProgress) + Send + Sync>;

/// Per-transfer cancellation and progress reporting.
///
/// A control belongs to one file transfer. Progress is made monotonic here so
/// network retries cannot make the UI jump backwards.
#[derive(Clone)]
pub struct TransferControl {
    cancel: Arc<AtomicBool>,
    progress: ProgressFn,
    last_reported: Arc<AtomicU64>,
}

impl TransferControl {
    pub fn new(cancel: Arc<AtomicBool>, progress: ProgressFn) -> Self {
        Self {
            cancel,
            progress,
            last_reported: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }

    pub fn uncancelled() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)), Arc::new(|_| {}))
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn check_cancelled(&self) -> Result<(), TransportError> {
        if self.is_cancelled() {
            Err(TransportError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn report(&self, transferred: u64, total: u64) {
        let transferred = transferred.min(total);
        let mut previous = self.last_reported.load(Ordering::Relaxed);
        loop {
            if previous != u64::MAX && transferred <= previous {
                return;
            }
            match self.last_reported.compare_exchange_weak(
                previous,
                transferred,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    (self.progress)(TransferProgress { transferred, total });
                    return;
                }
                Err(current) => previous = current,
            }
        }
    }

    pub fn transferred(&self) -> u64 {
        match self.last_reported.load(Ordering::Relaxed) {
            u64::MAX => 0,
            value => value,
        }
    }
}

impl Default for TransferControl {
    fn default() -> Self {
        Self::uncancelled()
    }
}

impl From<String> for TransportError {
    fn from(value: String) -> Self {
        TransportError::Other(value)
    }
}

impl From<&str> for TransportError {
    fn from(value: &str) -> Self {
        TransportError::Other(value.to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub relative_path: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone)]
pub struct ObjectHead {
    pub size: u64,
    pub mtime: Option<u64>,
}

/// Object-safe path-based storage interface used by the sync engine.
pub trait BackupTransport: Send + Sync {
    fn test_connection(&self) -> Result<(), TransportError>;
    fn upload_file(
        &self,
        relative_path: &str,
        local_path: &Path,
        metadata: &FileMetadata,
    ) -> Result<(), TransportError>;
    fn upload_file_with(
        &self,
        relative_path: &str,
        local_path: &Path,
        metadata: &FileMetadata,
        control: &TransferControl,
    ) -> Result<(), TransportError> {
        control.check_cancelled()?;
        control.report(0, metadata.size);
        self.upload_file(relative_path, local_path, metadata)?;
        control.check_cancelled()?;
        control.report(metadata.size, metadata.size);
        Ok(())
    }
    fn download_file(
        &self,
        relative_path: &str,
        destination_path: &Path,
    ) -> Result<FileMetadata, TransportError>;
    fn download_file_with(
        &self,
        relative_path: &str,
        destination_path: &Path,
        control: &TransferControl,
    ) -> Result<FileMetadata, TransportError> {
        control.check_cancelled()?;
        let metadata = self.download_file(relative_path, destination_path)?;
        control.check_cancelled()?;
        control.report(metadata.size, metadata.size);
        Ok(metadata)
    }
    fn list_files(&self) -> Result<Vec<RemoteFile>, TransportError>;
    fn head_file(&self, relative_path: &str) -> Result<Option<ObjectHead>, TransportError>;
    fn delete_file(&self, relative_path: &str) -> Result<(), TransportError>;
}

pub fn build(cfg: &Config, s3_secret: &str) -> Result<Arc<dyn BackupTransport>, String> {
    match config::transport_kind(cfg) {
        Some(config::TransportKind::S3) => {
            if cfg.s3_endpoint.trim().is_empty()
                || cfg.s3_bucket.trim().is_empty()
                || cfg.s3_access_key.trim().is_empty()
                || s3_secret.is_empty()
            {
                return Err(
                    "S3 transport requires endpoint, bucket, access key, and secret".into(),
                );
            }
            Ok(Arc::new(S3Transport::new(cfg, s3_secret)?))
        }
        None => {
            let configured = cfg.transport.trim();
            if configured.is_empty() {
                Err("Not paired for S3. Pair again to get storage credentials.".into())
            } else {
                Err(format!("Unsupported backup transport: {configured}"))
            }
        }
    }
}

/// Prove that newly issued credentials can write, read metadata, and clean up
/// before the desktop activates them locally.
pub fn validate_candidate(cfg: &Config, s3_secret: &str) -> Result<(), String> {
    let transport = build(cfg, s3_secret)
        .map_err(|err| format!("Approved storage configuration is invalid: {err}"))?;
    transport
        .test_connection()
        .map_err(|err| format!("Approved storage could not be listed: {err}"))?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let relative = format!(
        ".backupsynctool-validation/{}-{nonce}.probe",
        std::process::id()
    );
    let local = std::env::temp_dir().join(format!(
        "backupsynctool-validation-{}-{nonce}.probe",
        std::process::id()
    ));
    let contents = b"Backup Sync Tool credential validation";
    std::fs::write(&local, contents)
        .map_err(|err| format!("Could not create credential validation probe: {err}"))?;
    let metadata = FileMetadata {
        size: contents.len() as u64,
        mtime: 0,
    };
    let upload = transport.upload_file(&relative, &local, &metadata);
    let _ = std::fs::remove_file(&local);
    upload.map_err(|err| format!("Approved storage could not write a probe: {err}"))?;

    let verified = match transport.head_file(&relative) {
        Ok(head) => head.is_some_and(|head| head.size == metadata.size),
        Err(err) => {
            let _ = transport.delete_file(&relative);
            return Err(format!(
                "Approved storage probe could not be verified: {err}"
            ));
        }
    };
    if !verified {
        let _ = transport.delete_file(&relative);
        return Err("Approved storage probe verification returned the wrong size.".into());
    }
    transport
        .delete_file(&relative)
        .map_err(|err| format!("Approved storage probe could not be removed: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct DummyTransport;

    impl BackupTransport for DummyTransport {
        fn test_connection(&self) -> Result<(), TransportError> {
            Ok(())
        }
        fn upload_file(
            &self,
            _relative_path: &str,
            _local_path: &Path,
            _metadata: &FileMetadata,
        ) -> Result<(), TransportError> {
            Ok(())
        }
        fn download_file(
            &self,
            _relative_path: &str,
            _destination_path: &Path,
        ) -> Result<FileMetadata, TransportError> {
            Ok(FileMetadata::default())
        }
        fn list_files(&self) -> Result<Vec<RemoteFile>, TransportError> {
            Ok(Vec::new())
        }
        fn head_file(&self, _relative_path: &str) -> Result<Option<ObjectHead>, TransportError> {
            Ok(None)
        }
        fn delete_file(&self, _relative_path: &str) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[test]
    fn trait_object_is_object_safe() {
        let transport: Arc<dyn BackupTransport> = Arc::new(DummyTransport);
        assert!(transport.test_connection().is_ok());
        assert!(transport.list_files().unwrap().is_empty());
    }

    #[test]
    fn unknown_transport_is_rejected() {
        let mut cfg = Config::default();
        cfg.transport = "future-storage".to_string();
        match build(&cfg, "") {
            Ok(_) => panic!("unknown transport was accepted"),
            Err(err) => assert!(err.contains("Unsupported backup transport")),
        }
    }

    #[test]
    fn empty_transport_is_rejected() {
        let cfg = Config::default();
        match build(&cfg, "secret") {
            Ok(_) => panic!("empty transport was accepted"),
            Err(err) => assert!(err.contains("Not paired for S3")),
        }
    }

    #[test]
    fn transfer_progress_is_monotonic_across_retries() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_callback = seen.clone();
        let control = TransferControl::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |progress| seen_callback.lock().unwrap().push(progress.transferred)),
        );

        control.report(0, 10);
        control.report(6, 10);
        control.report(2, 10);
        control.report(6, 10);
        control.report(10, 10);

        assert_eq!(*seen.lock().unwrap(), vec![0, 6, 10]);
    }

    #[test]
    fn cancelled_control_has_a_distinct_error() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let control = TransferControl::new(cancelled, Arc::new(|_| {}));
        let error = control.check_cancelled().unwrap_err();
        assert!(error.is_cancelled());
        assert!(!error.is_auth_failed());
    }
}
