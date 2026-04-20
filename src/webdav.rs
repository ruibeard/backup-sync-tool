// webdav.rs — minimal blocking WebDAV client over ureq

#![allow(dead_code)]

use crate::config::Config;
use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub href: String,
    pub is_collection: bool,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

fn basic_auth(user: &str, pass: &str) -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    format!("Basic {}", B64.encode(format!("{user}:{pass}").as_bytes()))
}

fn validate_https(url: &str) -> Result<(), String> {
    if url.trim().to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err("Server URL must use https://".to_string())
    }
}

pub fn test_connection(cfg: &Config, password: &str) -> Result<(), String> {
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
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if status < 400 {
        Ok(())
    } else {
        Err(format!("Server returned HTTP {}", status))
    }
}

pub fn list_folders(cfg: &Config, password: &str, folder_url: &str) -> Result<Vec<String>, String> {
    validate_https(&cfg.webdav_url)?;
    let entries = list_entries(cfg, password, folder_url, 1)?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.is_collection)
        .map(|entry| entry.href)
        .collect())
}

pub fn list_entries_recursive(
    cfg: &Config,
    password: &str,
    folder_url: &str,
) -> Result<Vec<RemoteFile>, String> {
    validate_https(&cfg.webdav_url)?;
    let mut queue = VecDeque::from([folder_url.trim_end_matches('/').to_string() + "/"]);
    let mut seen_dirs = HashSet::new();
    let mut all = Vec::new();

    while let Some(current) = queue.pop_front() {
        let current_key = current.trim_end_matches('/').to_string();
        if !seen_dirs.insert(current_key) {
            continue;
        }

        let entries = list_entries(cfg, password, &current, 1)?;
        for entry in entries {
            if entry.is_collection {
                let next = entry.href.trim_end_matches('/').to_string() + "/";
                queue.push_back(next);
            }
            all.push(entry);
        }
    }

    Ok(all)
}

pub fn get_file(cfg: &Config, password: &str, remote_url: &str) -> Result<Vec<u8>, String> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    let mut reader = agent()
        .request("GET", remote_url)
        .set("Authorization", &auth)
        .call()
        .map_err(|e| e.to_string())?
        .into_reader();
    let mut data = Vec::new();
    reader.read_to_end(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}

pub fn put_file<R: Read>(
    cfg: &Config,
    password: &str,
    remote_url: &str,
    reader: R,
    content_length: u64,
) -> Result<(), String> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    agent()
        .request("PUT", remote_url)
        .set("Authorization", &auth)
        .set("Content-Length", &content_length.to_string())
        .send(reader)
        .map_err(|e| e.to_string())
        .and_then(|r| {
            if r.status() < 400 {
                Ok(())
            } else {
                Err(format!("PUT returned HTTP {}", r.status()))
            }
        })
}

pub fn set_sar_last_modified(
    cfg: &Config,
    password: &str,
    remote_url: &str,
    modified_epoch: u64,
) -> Result<(), String> {
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
        .map_err(|e| e.to_string())?;

    if response.status() >= 400 {
        return Err(format!("PROPPATCH returned HTTP {}", response.status()));
    }

    let xml = response.into_string().map_err(|e| e.to_string())?;
    if xml.contains("<ns1:lastmodified/>") && xml.contains("200 OK") {
        Ok(())
    } else {
        Err("PROPPATCH did not confirm SAR:lastmodified".to_string())
    }
}

pub fn mkcol(cfg: &Config, password: &str, remote_url: &str) -> Result<(), String> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    match agent()
        .request("MKCOL", remote_url)
        .set("Authorization", &auth)
        .call()
    {
        Ok(resp) => {
            let status = resp.status();
            if status < 400 || status == 405 {
                Ok(())
            } else {
                Err(format!("MKCOL returned HTTP {}", status))
            }
        }
        Err(ureq::Error::Status(405, _)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn parse_propfind_folders(xml: &str, base_url: &str) -> Vec<String> {
    parse_propfind_entries(xml, base_url)
        .into_iter()
        .filter(|entry| entry.is_collection)
        .map(|entry| entry.href)
        .collect()
}

fn list_entries(
    cfg: &Config,
    password: &str,
    folder_url: &str,
    depth: u32,
) -> Result<Vec<RemoteFile>, String> {
    validate_https(&cfg.webdav_url)?;
    let auth = basic_auth(&cfg.username, password);
    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:displayname/></D:prop></D:propfind>"#;
    let response = agent()
        .request("PROPFIND", folder_url)
        .set("Authorization", &auth)
        .set("Depth", if depth == 0 { "0" } else { "1" })
        .set("Content-Type", "application/xml")
        .send_string(body)
        .map_err(|e| e.to_string())?;

    if response.status() >= 400 {
        return Err(format!("PROPFIND returned HTTP {}", response.status()));
    }

    let xml = response.into_string().map_err(|e| e.to_string())?;
    Ok(parse_propfind_entries(&xml, folder_url))
}

fn parse_propfind_entries(xml: &str, base_url: &str) -> Vec<RemoteFile> {
    let mut entries = Vec::new();
    let xml_lower = xml.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel_start) = find_response_start(&xml_lower[search_from..]) {
        let start = search_from + rel_start;
        let next_search = start + 1;
        let end = match find_response_start(&xml_lower[next_search..]) {
            Some(rel_end) => next_search + rel_end,
            None => xml.len(),
        };
        let block = &xml[start..end];
        let block_lower = &xml_lower[start..end];

        if let Some(href) = extract_href(block, block_lower) {
            let href = absolutize_href(base_url, &decode_href(&href));
            let is_collection =
                block_lower.contains("<d:collection") || block_lower.contains("<collection");
            if href.trim_end_matches('/') != base_url.trim_end_matches('/') {
                entries.push(RemoteFile {
                    href,
                    is_collection,
                });
            }
        }

        search_from = end;
    }
    entries
}

fn absolutize_href(base_url: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }

    let trimmed = href.trim();
    if trimmed.starts_with('/') {
        if let Some(idx) = base_url.find("//") {
            let after_scheme = idx + 2;
            if let Some(path_idx) = base_url[after_scheme..].find('/') {
                let origin = &base_url[..after_scheme + path_idx];
                return format!("{origin}{trimmed}");
            }
        }
    }

    let prefix = base_url.trim_end_matches('/');
    format!("{prefix}/{}", trimmed.trim_start_matches('/'))
}

fn find_response_start(xml_lower: &str) -> Option<usize> {
    let a = xml_lower.find("<d:response");
    let b = xml_lower.find("<response");
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn extract_href(block: &str, block_lower: &str) -> Option<String> {
    for (open, close) in [("<d:href>", "</d:href>"), ("<href>", "</href>")] {
        if let Some(start) = block_lower.find(open) {
            let rest = &block[start + open.len()..];
            let rest_lower = &block_lower[start + open.len()..];
            if let Some(end) = rest_lower.find(close) {
                return Some(rest[..end].trim().to_string());
            }
        }
    }
    None
}

fn decode_href(href: &str) -> String {
    let bytes = href.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &href[i + 1..i + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
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
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
        "Dec",
    ][(month - 1) as usize];
    format!(
        "{weekday}, {day:02} {month} {year:04} {hour:02}:{minute:02}:{second:02} UTC"
    )
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
