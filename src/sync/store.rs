//! Whole-file PUT/GET against signed URLs from the control plane.
//!
//! The device holds no store keys. Laravel signs short-lived URLs for one
//! object key (`{destination}/{relative path}`); bytes go straight to the
//! object store and never sit whole in memory.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use ureq::Agent;

/// Largest object one signed PUT can carry (S3 single-request limit).
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FileStore {
    agent: Agent,
}

impl FileStore {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(8))
                .timeout_read(Duration::from_secs(60))
                .timeout_write(Duration::from_secs(60))
                // Keep one warm connection per transfer worker.
                .max_idle_connections_per_host(16)
                .build(),
        }
    }

    /// Stream `path` to the store with one PUT. Returns the object version
    /// (`x-amz-version-id`) when the store has versioning on.
    pub fn put_file(&self, url: &str, path: &Path) -> Result<Option<String>, String> {
        let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let len = file
            .metadata()
            .map_err(|e| format!("stat {}: {e}", path.display()))?
            .len();
        if len > MAX_FILE_BYTES {
            return Err(format!("{} is larger than 5 GiB", path.display()));
        }
        let resp = self
            .agent
            .put(url)
            .set("Content-Type", "application/octet-stream")
            .set("Content-Length", &len.to_string())
            // `take` keeps the body at the declared length if the file grows.
            .send(file.take(len))
            .map_err(|e| map_ureq_err("file PUT", e))?;
        Ok(resp
            .header("x-amz-version-id")
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string))
    }

    /// Stream one object into `dest_tmp` while hashing it, then check the
    /// hash against `sha256_hex` (skipped when it is empty). Returns the
    /// bytes written. On any error the temp file is removed.
    pub fn get_file(&self, url: &str, dest_tmp: &Path, sha256_hex: &str) -> Result<u64, String> {
        let resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| map_ureq_err("file GET", e))?;
        let result = stream_to_file(resp.into_reader(), dest_tmp, sha256_hex);
        if result.is_err() {
            let _ = fs::remove_file(dest_tmp);
        }
        result
    }
}

fn stream_to_file(mut reader: impl Read, dest: &Path, sha256_hex: &str) -> Result<u64, String> {
    let mut file = fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut written = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("file GET body: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("write {}: {e}", dest.display()))?;
        written += n as u64;
    }
    file.sync_all().ok();
    let want = sha256_hex.trim();
    if !want.is_empty() {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("file hash mismatch: expected {want}, got {got}"));
        }
    }
    Ok(written)
}

fn map_ureq_err(op: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(404, _) => format!("{op}: file missing in store"),
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            format!("{op} HTTP {code}: {snippet}")
        }
        // Signed URLs carry a signature; never put the URL in a log line.
        ureq::Error::Transport(t) => match t.message() {
            Some(msg) => format!("{op}: {}: {msg}", t.kind()),
            None => format!("{op}: {}", t.kind()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bst-store-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One-shot HTTP server. Replies with `status_line`, `extra` headers and
    /// `body`. Sends back the request line, headers and PUT body it read.
    fn serve_once(
        status_line: &'static str,
        extra: &'static str,
        body: Vec<u8>,
    ) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/obj", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
            }
            let mut got = vec![0u8; content_length];
            reader.read_exact(&mut got).unwrap();
            let mut out = stream;
            write!(
                out,
                "HTTP/1.1 {status_line}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            out.write_all(&body).unwrap();
            let _ = tx.send((head, got));
        });
        (url, rx)
    }

    #[test]
    fn put_streams_body_with_content_length_and_returns_version() {
        let dir = temp_dir("put");
        let file = dir.join("a.bin");
        let data: Vec<u8> = (0..300_000u32).map(|n| n as u8).collect();
        fs::write(&file, &data).unwrap();
        let (url, rx) = serve_once("200 OK", "x-amz-version-id: v-123\r\n", Vec::new());
        let version = FileStore::new().put_file(&url, &file).unwrap();
        assert_eq!(version.as_deref(), Some("v-123"));
        let (head, body) = rx.recv().unwrap();
        assert!(head.to_ascii_lowercase().contains("content-length: 300000"));
        assert!(!head.to_ascii_lowercase().contains("transfer-encoding"));
        assert_eq!(body, data);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn put_without_version_header_returns_none() {
        let dir = temp_dir("put-nover");
        let file = dir.join("empty.txt");
        fs::write(&file, b"").unwrap();
        let (url, _rx) = serve_once("200 OK", "", Vec::new());
        assert_eq!(FileStore::new().put_file(&url, &file).unwrap(), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn get_checks_hash_and_cleans_temp_on_mismatch() {
        let dir = temp_dir("get");
        let tmp = dir.join(".1.bst-tmp");
        let body = b"hello world".to_vec();
        let sha = hex::encode(Sha256::digest(&body));

        let (url, _rx) = serve_once("200 OK", "", body.clone());
        let n = FileStore::new().get_file(&url, &tmp, &sha).unwrap();
        assert_eq!(n, 11);
        assert_eq!(fs::read(&tmp).unwrap(), body);
        fs::remove_file(&tmp).unwrap();

        let (url, _rx) = serve_once("200 OK", "", body);
        let err = FileStore::new()
            .get_file(&url, &tmp, &"0".repeat(64))
            .unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_never_contain_the_url() {
        let dir = temp_dir("err");
        let tmp = dir.join("x.bst-tmp");
        let (url, _rx) = serve_once("404 Not Found", "", Vec::new());
        let err = FileStore::new().get_file(&url, &tmp, "").unwrap_err();
        assert!(err.contains("missing in store"));
        assert!(!err.contains("127.0.0.1"));
        let _ = fs::remove_dir_all(dir);
    }
}
