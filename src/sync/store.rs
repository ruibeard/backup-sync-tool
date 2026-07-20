//! S3-compatible chunk PUT/GET via blocking ureq + SigV4 (Garage/MinIO path-style).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use ureq::Agent;

type HmacSha256 = Hmac<Sha256>;

const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone)]
pub struct ChunkStoreClient {
    endpoint: String,
    region: String,
    bucket: String,
    prefix: String,
    access_key: String,
    secret_key: String,
    path_style: bool,
    agent: Agent,
}

impl ChunkStoreClient {
    pub fn new(
        endpoint: &str,
        region: &str,
        bucket: &str,
        prefix: &str,
        access_key: &str,
        secret_key: &str,
        path_style: bool,
    ) -> Self {
        let mut prefix = prefix.trim().to_string();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            region: if region.trim().is_empty() {
                "garage".into()
            } else {
                region.trim().to_string()
            },
            bucket: bucket.trim().to_string(),
            prefix,
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            path_style,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(8))
                .timeout_read(Duration::from_secs(60))
                .timeout_write(Duration::from_secs(60))
                .build(),
        }
    }

    /// `{chunk_prefix}chunks/{sha256[0:2]}/{sha256}`
    pub fn object_key(&self, sha256_hex: &str) -> Result<String, String> {
        let hash = sha256_hex.trim().to_ascii_lowercase();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid chunk sha256: {sha256_hex}"));
        }
        Ok(format!("{}chunks/{}/{}", self.prefix, &hash[..2], hash))
    }

    pub fn put_chunk(&self, sha256_hex: &str, data: &[u8]) -> Result<(), String> {
        let key = self.object_key(sha256_hex)?;
        let payload_hash = hex::encode(Sha256::digest(data));
        let (url, host, canonical_uri) = self.request_target("PUT", &key)?;
        let amz_date = amz_date_now()?;
        let date_stamp = &amz_date[..8];
        let auth = sign_request(
            "PUT",
            &canonical_uri,
            "",
            &host,
            &amz_date,
            date_stamp,
            &self.region,
            &payload_hash,
            &self.access_key,
            &self.secret_key,
        )?;
        let resp = self
            .agent
            .put(&url)
            .set("Host", &host)
            .set("x-amz-content-sha256", &payload_hash)
            .set("x-amz-date", &amz_date)
            .set("Authorization", &auth)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(data)
            .map_err(|e| map_ureq_err("chunk PUT", e))?;
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(format!("chunk PUT HTTP {status} for {key}"));
        }
        Ok(())
    }

    pub fn get_chunk(&self, sha256_hex: &str) -> Result<Vec<u8>, String> {
        let key = self.object_key(sha256_hex)?;
        let (url, host, canonical_uri) = self.request_target("GET", &key)?;
        let amz_date = amz_date_now()?;
        let date_stamp = &amz_date[..8];
        let auth = sign_request(
            "GET",
            &canonical_uri,
            "",
            &host,
            &amz_date,
            date_stamp,
            &self.region,
            EMPTY_PAYLOAD_SHA256,
            &self.access_key,
            &self.secret_key,
        )?;
        let resp = self
            .agent
            .get(&url)
            .set("Host", &host)
            .set("x-amz-content-sha256", EMPTY_PAYLOAD_SHA256)
            .set("x-amz-date", &amz_date)
            .set("Authorization", &auth)
            .call()
            .map_err(|e| map_ureq_err("chunk GET", e))?;
        let status = resp.status();
        if status == 404 {
            return Err(format!("chunk missing in store: {sha256_hex}"));
        }
        if !(200..300).contains(&status) {
            return Err(format!("chunk GET HTTP {status} for {key}"));
        }
        let mut bytes = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("chunk GET body: {e}"))?;
        let got = hex::encode(Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(sha256_hex.trim()) {
            return Err(format!(
                "chunk hash mismatch: expected {sha256_hex}, got {got}"
            ));
        }
        Ok(bytes)
    }

    fn request_target(&self, _method: &str, key: &str) -> Result<(String, String, String), String> {
        let parsed = parse_endpoint(&self.endpoint)?;
        let encoded_key = encode_s3_path(key);
        let (url, canonical_uri, host_header) = if self.path_style {
            let path = format!("/{}/{}", self.bucket, encoded_key);
            let url = format!("{}{path}", parsed.base);
            (url, path, parsed.host_header)
        } else {
            let authority = match parsed.port {
                Some(port) => format!("{}.{}:{port}", self.bucket, parsed.host),
                None => format!("{}.{}", self.bucket, parsed.host),
            };
            let path = format!("/{encoded_key}");
            let url = format!("{}://{authority}{path}", parsed.scheme);
            (url, path, authority)
        };
        Ok((url, host_header, canonical_uri))
    }
}

#[derive(Debug)]
struct ParsedEndpoint {
    scheme: String,
    host: String,
    port: Option<u16>,
    host_header: String,
    base: String,
}

fn parse_endpoint(endpoint: &str) -> Result<ParsedEndpoint, String> {
    let raw = endpoint.trim().trim_end_matches('/');
    let (scheme, rest) = if let Some(rest) = raw.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = raw.strip_prefix("http://") {
        ("http", rest)
    } else {
        ("https", raw)
    };
    if rest.is_empty() || rest.contains('/') {
        return Err(format!("bad chunk_endpoint (unexpected path): {endpoint}"));
    }
    let (host, port) = if let Some((h, p)) = rest.rsplit_once(':') {
        // IPv6 literals are out of scope for desktop chunk endpoints.
        let port: u16 = p
            .parse()
            .map_err(|_| format!("bad chunk_endpoint port: {endpoint}"))?;
        (h.to_string(), Some(port))
    } else {
        (rest.to_string(), None)
    };
    if host.is_empty() {
        return Err(format!("chunk_endpoint missing host: {endpoint}"));
    }
    let host_header = match port {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let base = format!("{scheme}://{host_header}");
    Ok(ParsedEndpoint {
        scheme: scheme.into(),
        host,
        port,
        host_header,
        base,
    })
}

fn map_ureq_err(op: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            format!("{op} HTTP {code}: {snippet}")
        }
        other => format!("{op}: {other}"),
    }
}

fn amz_date_now() -> Result<String, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    Ok(format_amz_date(secs))
}

fn format_amz_date(unix_secs: u64) -> String {
    // Manual UTC formatting avoids pulling chrono.
    let days = unix_secs / 86400;
    let tod = unix_secs % 86400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}{month:02}{day:02}T{hour:02}{min:02}{sec:02}Z")
}

/// Howard Hinnant's civil_from_days (UTC).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[allow(clippy::too_many_arguments)]
fn sign_request(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    host: &str,
    amz_date: &str,
    date_stamp: &str,
    region: &str,
    payload_hash: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<String, String> {
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let canonical_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_hash}");
    let signing_key = signing_key(secret_key, date_stamp, region, "s3")?;
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    ))
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Result<Vec<u8>, String> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let k_region = hmac_sha256(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|e| format!("hmac key error: {e}"))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn encode_s3_path(path: &str) -> String {
    path.split('/')
        .map(encode_s3_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_s3_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_uses_prefix_and_hash_prefix() {
        let store = ChunkStoreClient::new(
            "http://127.0.0.1:9000",
            "garage",
            "backup-dev",
            "dest/abc/",
            "ak",
            "sk",
            true,
        );
        let hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        assert_eq!(
            store.object_key(hash).unwrap(),
            format!("dest/abc/chunks/de/{hash}")
        );
    }

    #[test]
    fn object_key_rejects_bad_hash() {
        let store = ChunkStoreClient::new(
            "http://127.0.0.1:9000",
            "garage",
            "b",
            "",
            "ak",
            "sk",
            true,
        );
        assert!(store.object_key("xyz").is_err());
    }

    #[test]
    fn sigv4_matches_fixed_vector() {
        let auth = sign_request(
            "PUT",
            "/backup-dev/dest/chunks/ab/abcd",
            "",
            "127.0.0.1:9000",
            "20260720T120000Z",
            "20260720",
            "garage",
            EMPTY_PAYLOAD_SHA256,
            "minioadmin",
            "minioadmin",
        )
        .unwrap();
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=minioadmin/20260720/garage/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=47c222acb51dc7b9bf33eb1d817be4e744f7698f3b3e0bbe86db361e44e6b7ba"
        );
    }

    #[test]
    fn format_amz_date_epoch() {
        assert_eq!(format_amz_date(0), "19700101T000000Z");
        assert_eq!(format_amz_date(1_784_548_800), "20260720T120000Z");
    }

    #[test]
    fn put_get_roundtrip_against_minio_when_configured() {
        let endpoint = match std::env::var("BACKUP_SYNC_MINIO_ENDPOINT") {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return,
        };
        let access = std::env::var("BACKUP_SYNC_MINIO_ACCESS").unwrap_or_else(|_| "minioadmin".into());
        let secret = std::env::var("BACKUP_SYNC_MINIO_SECRET").unwrap_or_else(|_| "minioadmin".into());
        let bucket = std::env::var("BACKUP_SYNC_MINIO_BUCKET").unwrap_or_else(|_| "backup-dev".into());
        let store = ChunkStoreClient::new(
            &endpoint,
            "us-east-1",
            &bucket,
            "test-prefix/",
            &access,
            &secret,
            true,
        );
        let data = b"option-h-chunk-roundtrip";
        let hash = hex::encode(Sha256::digest(data));
        store.put_chunk(&hash, data).expect("put");
        let got = store.get_chunk(&hash).expect("get");
        assert_eq!(got, data);
    }
}
