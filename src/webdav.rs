// webdav.rs — minimal blocking WebDAV client over ureq

use crate::config::Config;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fmt;
use std::io::Read;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum WebDavError {
    AuthFailed(u16),
    Http(u16, String),
    Other(String),
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub relative_path: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone, Default)]
struct PropfindEntry {
    href: String,
    is_collection: bool,
    size: u64,
    mtime: u64,
}

impl WebDavError {
    pub fn is_auth_failed(&self) -> bool {
        matches!(self, WebDavError::AuthFailed(_))
    }
}

impl fmt::Display for WebDavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebDavError::AuthFailed(status) => write!(f, "Server returned HTTP {status}"),
            WebDavError::Http(status, action) => write!(f, "{action} returned HTTP {status}"),
            WebDavError::Other(message) => f.write_str(message),
        }
    }
}

impl From<String> for WebDavError {
    fn from(value: String) -> Self {
        WebDavError::Other(value)
    }
}

impl From<&str> for WebDavError {
    fn from(value: &str) -> Self {
        WebDavError::Other(value.to_string())
    }
}

impl From<ureq::Error> for WebDavError {
    fn from(value: ureq::Error) -> Self {
        match value {
            ureq::Error::Status(status, _) if status == 401 => WebDavError::AuthFailed(status),
            ureq::Error::Status(status, _) => WebDavError::Http(status, "Request".to_string()),
            err => WebDavError::Other(err.to_string()),
        }
    }
}

fn http_error(status: u16, action: &str) -> WebDavError {
    if status == 401 {
        WebDavError::AuthFailed(status)
    } else {
        WebDavError::Http(status, action.to_string())
    }
}

fn agent() -> ureq::Agent {
    agent_with_timeout(Duration::from_secs(30))
}

/// Size-aware timeout so large PUTs/GETs are not killed by the 30s default while
/// ten smaller uploads are also in flight.
fn transfer_timeout(content_length: u64) -> Duration {
    // 2 minutes base + ~15s per MiB, capped at 2 hours.
    let mib = content_length.div_ceil(1024 * 1024);
    let secs = 120u64.saturating_add(mib.saturating_mul(15)).min(2 * 60 * 60);
    Duration::from_secs(secs.max(30))
}

fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

fn basic_auth(user: &str, pass: &str) -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    format!("Basic {}", B64.encode(format!("{user}:{pass}").as_bytes()))
}

fn validate_https(url: &str) -> Result<(), WebDavError> {
    if url.trim().to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err(WebDavError::Other(
            "Server URL must use https://".to_string(),
        ))
    }
}

pub fn test_connection(cfg: &Config, password: &str) -> Result<(), WebDavError> {
    validate_https(&cfg.webdav_url)?;
    let url = format!("{}/", cfg.webdav_url.trim_end_matches('/'));
    let auth = basic_auth(&cfg.username, password);
    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/></D:prop></D:propfind>"#;
    let resp = agent()
        .request("PROPFIND", &url)
        .set("Authorization", &auth)
        .set("Depth", "0")
        .set("Content-Type", "application/xml")
        .send_string(body)
        .map_err(WebDavError::from)?;
    let status = resp.status();
    if status < 400 {
        Ok(())
    } else {
        Err(http_error(status, "Server"))
    }
}

pub fn get_file(cfg: &Config, password: &str, remote_url: &str) -> Result<Vec<u8>, WebDavError> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    // Unknown size up front — allow a generous transfer window.
    let mut reader = agent_with_timeout(Duration::from_secs(30 * 60))
        .request("GET", remote_url)
        .set("Authorization", &auth)
        .call()
        .map_err(WebDavError::from)?
        .into_reader();
    let mut data = Vec::new();
    reader
        .read_to_end(&mut data)
        .map_err(|e| WebDavError::Other(e.to_string()))?;
    Ok(data)
}

pub fn put_file<R: Read>(
    cfg: &Config,
    password: &str,
    remote_url: &str,
    reader: R,
    content_length: u64,
) -> Result<(), WebDavError> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    agent_with_timeout(transfer_timeout(content_length))
        .request("PUT", remote_url)
        .set("Authorization", &auth)
        .set("Content-Length", &content_length.to_string())
        .send(reader)
        .map_err(WebDavError::from)
        .and_then(|r| {
            if r.status() < 400 {
                Ok(())
            } else {
                Err(http_error(r.status(), "PUT"))
            }
        })
}

pub fn set_sar_last_modified(
    cfg: &Config,
    password: &str,
    remote_url: &str,
    modified_epoch: u64,
) -> Result<(), WebDavError> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    let modified = UNIX_EPOCH + Duration::from_secs(modified_epoch);
    let modified = sar_http_date(modified);
    let body = format!(
        "<?xml version=\"1.0\"?><D:propertyupdate xmlns:D=\"DAV:\" xmlns:S=\"SAR:\"><D:set><D:prop><S:lastmodified>{modified}</S:lastmodified></D:prop></D:set></D:propertyupdate>"
    );
    let response = agent()
        .request("PROPPATCH", remote_url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/xml")
        .send_string(&body)
        .map_err(WebDavError::from)?;

    if response.status() >= 400 {
        return Err(http_error(response.status(), "PROPPATCH"));
    }

    let xml = response
        .into_string()
        .map_err(|e| WebDavError::Other(e.to_string()))?;
    if xml.contains("<ns1:lastmodified/>") && xml.contains("200 OK") {
        Ok(())
    } else {
        Err(WebDavError::Other(
            "PROPPATCH did not confirm SAR:lastmodified".to_string(),
        ))
    }
}

pub fn mkcol(cfg: &Config, password: &str, remote_url: &str) -> Result<(), WebDavError> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    match agent()
        .request("MKCOL", remote_url)
        .set("Authorization", &auth)
        .call()
    {
        Ok(resp) => {
            let status = resp.status();
            if status < 400 || status == 405 || status == 403 {
                Ok(())
            } else {
                Err(http_error(status, "MKCOL"))
            }
        }
        Err(ureq::Error::Status(405, _)) | Err(ureq::Error::Status(403, _)) => Ok(()),
        Err(err) => Err(WebDavError::from(err)),
    }
}

pub fn list_files_recursive(
    cfg: &Config,
    password: &str,
    remote_base_url: &str,
) -> Result<Vec<RemoteFile>, WebDavError> {
    validate_https(&cfg.webdav_url)?;

    let mut files = Vec::new();
    let mut queue = std::collections::VecDeque::from([ensure_trailing_slash(remote_base_url)]);
    let mut seen_dirs = std::collections::HashSet::new();
    let base_path = url_path(remote_base_url);

    while let Some(folder_url) = queue.pop_front() {
        let folder_url = ensure_trailing_slash(&folder_url);
        if !seen_dirs.insert(folder_url.clone()) {
            continue;
        }

        let entries = propfind_depth_one(cfg, password, &folder_url)?;
        let folder_path = url_path(&folder_url);
        for entry in entries {
            let href_url = absolute_href_url(&cfg.webdav_url, &entry.href);
            let href_path = url_path(&href_url);
            if same_webdav_path(&href_path, &folder_path) {
                continue;
            }

            if entry.is_collection {
                queue.push_back(ensure_trailing_slash(&href_url));
                continue;
            }

            let Some(relative_path) = relative_href_path(&base_path, &href_path) else {
                continue;
            };
            if relative_path.is_empty() {
                continue;
            }

            files.push(RemoteFile {
                relative_path,
                size: entry.size,
                mtime: entry.mtime,
            });
        }
    }

    Ok(files)
}

fn propfind_depth_one(
    cfg: &Config,
    password: &str,
    remote_url: &str,
) -> Result<Vec<PropfindEntry>, WebDavError> {
    let auth = basic_auth(&cfg.username, password);
    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:" xmlns:S="SAR:"><D:prop><D:resourcetype/><D:getcontentlength/><D:getlastmodified/><S:lastmodified/></D:prop></D:propfind>"#;
    let response = agent()
        .request("PROPFIND", remote_url)
        .set("Authorization", &auth)
        .set("Depth", "1")
        .set("Content-Type", "application/xml")
        .send_string(body)
        .map_err(WebDavError::from)?;

    if response.status() >= 400 {
        return Err(http_error(response.status(), "PROPFIND"));
    }

    let xml = response
        .into_string()
        .map_err(|e| WebDavError::Other(e.to_string()))?;
    Ok(parse_propfind_entries(&xml))
}

fn parse_propfind_entries(xml: &str) -> Vec<PropfindEntry> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut entries = Vec::new();
    let mut current: Option<PropfindEntry> = None;
    let mut text_target = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = local_name(e.name().as_ref());
                if tag == "response" {
                    current = Some(PropfindEntry::default());
                } else if let Some(entry) = current.as_mut() {
                    if tag == "collection" {
                        entry.is_collection = true;
                    }
                    if matches!(
                        tag.as_str(),
                        "href" | "getcontentlength" | "getlastmodified" | "lastmodified"
                    ) {
                        text_target = tag;
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if let Some(entry) = current.as_mut() {
                    if local_name(e.name().as_ref()) == "collection" {
                        entry.is_collection = true;
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(entry) = current.as_mut() {
                    let text = e.unescape().map(|cow| cow.into_owned()).unwrap_or_default();
                    match text_target.as_str() {
                        "href" => entry.href = text,
                        "getcontentlength" => entry.size = text.parse().unwrap_or(0),
                        "getlastmodified" | "lastmodified" => {
                            entry.mtime = parse_http_date_epoch(&text).unwrap_or(entry.mtime);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(e.name().as_ref());
                if tag == "response" {
                    if let Some(entry) = current.take() {
                        if !entry.href.is_empty() {
                            entries.push(entry);
                        }
                    }
                }
                if tag == text_target {
                    text_target.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    entries
}

fn local_name(name: &[u8]) -> String {
    let name = std::str::from_utf8(name).unwrap_or_default();
    name.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(name)
        .to_string()
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn absolute_href_url(base_url: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let origin = url_origin(base_url);
    if href.starts_with('/') {
        format!("{origin}{href}")
    } else {
        format!("{}/{}", origin.trim_end_matches('/'), href)
    }
}

fn url_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let host = rest.split('/').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

fn url_path(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let path = if let Some((_, rest)) = without_query.split_once("://") {
        rest.find('/').map(|idx| &rest[idx..]).unwrap_or("/")
    } else {
        without_query
    };
    normalise_webdav_path(&percent_decode(path))
}

fn same_webdav_path(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn relative_href_path(base_path: &str, href_path: &str) -> Option<String> {
    let base = base_path.trim_end_matches('/');
    let href = href_path.trim_start_matches('/');
    let base = base.trim_start_matches('/');
    let relative = href.strip_prefix(base)?.trim_start_matches('/');
    Some(relative.trim_end_matches('/').to_string())
}

fn normalise_webdav_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let mut out = String::new();
    let mut last_was_slash = false;
    for ch in replaced.chars() {
        if ch == '/' {
            if !last_was_slash {
                out.push(ch);
            }
            last_was_slash = true;
        } else {
            out.push(ch);
            last_was_slash = false;
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else if out.starts_with('/') {
        out
    } else {
        format!("/{out}")
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_http_date_epoch(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    parts.next()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i32>().ok()?;
    let mut time = parts.next()?.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time.next()?.parse::<i64>().ok()?;
    Some(
        days_from_civil(year, month, day) as u64 * 86_400
            + hour as u64 * 3_600
            + minute as u64 * 60
            + second as u64,
    )
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

fn sar_http_date(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let second = sod % 60;
    let (year, month, day) = civil_from_days(days);
    let weekday = ((days + 4).rem_euclid(7)) as usize;
    let weekday = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][weekday];
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(month - 1) as usize];
    format!("{weekday}, {day:02} {month} {year:04} {hour:02}:{minute:02}:{second:02} UTC")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::secret;

    fn test_config() -> (Config, String) {
        let cfg_text = std::fs::read_to_string("backupsynctool.json").expect("config");
        let cfg: Config = serde_json::from_str(&cfg_text).expect("json");
        let pass = secret::decrypt(&cfg.password_enc).expect("decrypt");
        (cfg, pass)
    }

    #[test]
    fn test_connection_works() {
        let (cfg, pass) = test_config();
        test_connection(&cfg, &pass).expect("connection");
    }

    #[test]
    fn mkcol_existing_folder_is_not_auth_failure() {
        let (cfg, pass) = test_config();
        let url = format!(
            "{}/{}/St Johns Cambridge TEST APPs 2025.2.0.0/",
            cfg.webdav_url.trim_end_matches('/'),
            cfg.remote_folder.trim_matches('/')
        );
        mkcol(&cfg, &pass, &url).expect("existing folder mkcol should succeed");
    }

    #[test]
    fn mkcol_on_file_path_is_not_auth_failure() {
        let (cfg, pass) = test_config();
        let url = format!(
            "{}/{}/St Johns Cambridge TEST APPs 2025.2.0.0/american.adm/",
            cfg.webdav_url.trim_end_matches('/'),
            cfg.remote_folder.trim_matches('/')
        );
        let err = mkcol(&cfg, &pass, &url).unwrap_err();
        assert!(!err.is_auth_failed());
    }
}
