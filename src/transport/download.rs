// Shared streamed download: write to destination.part, then atomically rename.

use crate::transport::TransportError;
use std::fs::{self, File, FileTimes};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const STREAM_BUF: usize = 64 * 1024;

pub fn stream_to_atomic_file<R: Read>(
    mut reader: R,
    destination: &Path,
    expected_size: Option<u64>,
) -> Result<u64, TransportError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| TransportError::Other(e.to_string()))?;
    }

    let temp_path = part_path(destination);
    let result = (|| {
        let mut file =
            File::create(&temp_path).map_err(|e| TransportError::Other(e.to_string()))?;
        let mut buf = vec![0u8; STREAM_BUF];
        let mut written = 0u64;

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| TransportError::Other(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| TransportError::Other(e.to_string()))?;
            written += n as u64;
        }

        file.flush()
            .map_err(|e| TransportError::Other(e.to_string()))?;
        drop(file);

        if let Some(expected) = expected_size {
            if written != expected {
                return Err(TransportError::Other(format!(
                    "Download size mismatch: got {written}, expected {expected}"
                )));
            }
        }

        replace_file(&temp_path, destination).map_err(|e| TransportError::Other(e.to_string()))?;
        Ok(written)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

pub fn apply_mtime(path: &Path, mtime: u64) -> Result<(), TransportError> {
    if mtime == 0 {
        return Ok(());
    }
    let modified = UNIX_EPOCH + Duration::from_secs(mtime);
    let file = File::options()
        .write(true)
        .open(path)
        .map_err(|e| TransportError::Other(e.to_string()))?;
    let times = FileTimes::new().set_modified(modified);
    file.set_times(times)
        .map_err(|e| TransportError::Other(e.to_string()))
}

fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".backupsynctool.part");
    destination.with_file_name(name)
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
    fs::rename(source, destination)
}
