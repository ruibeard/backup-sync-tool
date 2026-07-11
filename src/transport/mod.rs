// transport/mod.rs — object-safe backup storage backends (S3)

mod download;
mod s3;

use crate::config::{self, Config};
use std::path::Path;
use std::sync::Arc;

pub use s3::S3Transport;

#[derive(Debug, Clone)]
pub enum TransportError {
    AuthFailed(String),
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
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::AuthFailed(message) => write!(f, "{message}"),
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
    fn download_file(
        &self,
        relative_path: &str,
        destination_path: &Path,
    ) -> Result<FileMetadata, TransportError>;
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
            if configured.is_empty() || configured.eq_ignore_ascii_case("webdav") {
                Err("WebDAV is no longer supported. Pair again for S3 storage.".into())
            } else {
                Err(format!("Unsupported backup transport: {configured}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
    fn legacy_webdav_config_is_rejected() {
        let mut cfg = Config::default();
        cfg.transport = "webdav".into();
        match build(&cfg, "secret") {
            Ok(_) => panic!("webdav was accepted"),
            Err(err) => assert!(err.contains("WebDAV is no longer supported")),
        }
    }
}
