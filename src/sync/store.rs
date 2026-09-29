//! Whole-file PUT/GET against signed URLs from the control plane.
//!
//! The device holds no store keys. Laravel signs short-lived URLs for one
//! object key (`{customer}/{relative path}`); bytes go straight to the
//! object store and never sit whole in memory. A single-part object's ETag is
//! the MD5 of its bytes, so MD5 is the hash used everywhere here.

use md5::{Digest, Md5};
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

    /// Stream `path` to the store with one PUT. Returns the new ETag.
    pub fn put_file(&self, url: &str, path: &Path) -> Result<String, String> {
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
        match resp.header("etag").map(|v| v.trim().trim_matches('"')) {
            Some(etag) if !etag.is_empty() => Ok(etag.to_string()),
            // The store sent no ETag: hash the file, as the store did.
            _ => hash_file(path).map(|(md5, _)| md5),
        }
    }

    /// Delete one object. A missing object counts as deleted.
    pub fn delete(&self, url: &str) -> Result<(), String> {
        match self.agent.delete(url).call() {
            Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(map_ureq_err("file DELETE", e)),
        }
    }

    /// Stream one object into `dest_tmp` and check it against `size` and
    /// `md5_hex` (the hash check is skipped when it is empty). On any
    /// error the temp file is removed.
    pub fn get_file(
        &self,
        url: &str,
        dest_tmp: &Path,
        size: u64,
        md5_hex: &str,
    ) -> Result<(), String> {
        let resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| map_ureq_err("file GET", e))?;
        let result = stream_to_file(resp.into_reader(), dest_tmp, size, md5_hex);
        if result.is_err() {
            let _ = fs::remove_file(dest_tmp);
        }
        result
    }
}

/// MD5 and size of a file, streamed from disk.
pub fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    copy_hashed(file, std::io::sink()).map_err(|e| format!("read {}: {e}", path.display()))
}

fn stream_to_file(reader: impl Read, dest: &Path, size: u64, md5_hex: &str) -> Result<(), String> {
    let mut file = fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let (got, written) = copy_hashed(reader, &mut file)
        .map_err(|e| format!("file GET into {}: {e}", dest.display()))?;
    file.sync_all().ok();
    if written != size {
        return Err(format!("size mismatch: got {written}, expected {size}"));
    }
    let want = md5_hex.trim();
    if !want.is_empty() && !got.eq_ignore_ascii_case(want) {
        return Err(format!("file hash mismatch: expected {want}, got {got}"));
    }
    Ok(())
}

/// Copy `reader` into `writer`, returning the MD5 hex and byte count.
fn copy_hashed(mut reader: impl Read, mut writer: impl Write) -> std::io::Result<(String, u64)> {
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
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
    fn put_streams_body_with_content_length_and_returns_etag() {
        let dir = temp_dir("put");
        let file = dir.join("a.bin");
        let data: Vec<u8> = (0..300_000u32).map(|n| n as u8).collect();
        fs::write(&file, &data).unwrap();
        let (url, rx) = serve_once("200 OK", "ETag: \"e-123\"\r\n", Vec::new());
        let etag = FileStore::new().put_file(&url, &file).unwrap();
        assert_eq!(etag, "e-123");
        let (head, body) = rx.recv().unwrap();
        assert!(head.to_ascii_lowercase().contains("content-length: 300000"));
        assert!(!head.to_ascii_lowercase().contains("transfer-encoding"));
        assert_eq!(body, data);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn put_without_etag_header_hashes_the_file() {
        let dir = temp_dir("put-nover");
        let file = dir.join("empty.txt");
        fs::write(&file, b"").unwrap();
        let (url, _rx) = serve_once("200 OK", "", Vec::new());
        assert_eq!(
            FileStore::new().put_file(&url, &file).unwrap(),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn get_checks_size_and_hash_and_cleans_temp_on_mismatch() {
        let dir = temp_dir("get");
        let tmp = dir.join(".1.bst-tmp");
        let body = b"hello world".to_vec();
        let sha = hex::encode(Md5::digest(&body));

        let (url, _rx) = serve_once("200 OK", "", body.clone());
        FileStore::new().get_file(&url, &tmp, 11, &sha).unwrap();
        assert_eq!(fs::read(&tmp).unwrap(), body);
        fs::remove_file(&tmp).unwrap();

        let (url, _rx) = serve_once("200 OK", "", body.clone());
        let err = FileStore::new()
            .get_file(&url, &tmp, 11, &"0".repeat(64))
            .unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
        assert!(!tmp.exists());

        let (url, _rx) = serve_once("200 OK", "", body);
        let err = FileStore::new().get_file(&url, &tmp, 12, "").unwrap_err();
        assert!(err.contains("size mismatch"), "{err}");
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_never_contain_the_url() {
        let dir = temp_dir("err");
        let tmp = dir.join("x.bst-tmp");
        let (url, _rx) = serve_once("404 Not Found", "", Vec::new());
        let err = FileStore::new().get_file(&url, &tmp, 0, "").unwrap_err();
        assert!(err.contains("missing in store"));
        assert!(!err.contains("127.0.0.1"));
        let _ = fs::remove_dir_all(dir);
    }
}
