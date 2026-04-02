// webdav.rs — minimal blocking WebDAV client over ureq

#![allow(dead_code)]

use crate::config::Config;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

fn basic_auth(user: &str, pass: &str) -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    format!("Basic {}", B64.encode(format!("{user}:{pass}").as_bytes()))
}

pub fn test_connection(cfg: &Config, password: &str) -> Result<(), String> {
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
    let auth = basic_auth(&cfg.username, password);
    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:displayname/></D:prop></D:propfind>"#;
    let response = agent()
        .request("PROPFIND", folder_url)
        .set("Authorization", &auth)
        .set("Depth", "1")
        .set("Content-Type", "application/xml")
        .send_string(body)
        .map_err(|e| e.to_string())?;

    if response.status() >= 400 {
        return Err(format!("PROPFIND returned HTTP {}", response.status()));
    }

    let xml = response.into_string().map_err(|e| e.to_string())?;
    Ok(parse_propfind_folders(&xml, folder_url))
}

pub fn put_file(cfg: &Config, password: &str, remote_url: &str, data: &[u8]) -> Result<(), String> {
    let auth = basic_auth(&cfg.username, password);
    agent()
        .request("PUT", remote_url)
        .set("Authorization", &auth)
        .send_bytes(data)
        .map_err(|e| e.to_string())
        .and_then(|r| {
            if r.status() < 400 {
                Ok(())
            } else {
                Err(format!("PUT returned HTTP {}", r.status()))
            }
        })
}

pub fn mkcol(cfg: &Config, password: &str, remote_url: &str) -> Result<(), String> {
    let auth = basic_auth(&cfg.username, password);
    let status = agent()
        .request("MKCOL", remote_url)
        .set("Authorization", &auth)
        .call()
        .map_err(|e| e.to_string())?
        .status();
    if status < 400 || status == 405 {
        Ok(())
    } else {
        Err(format!("MKCOL returned HTTP {}", status))
    }
}

fn parse_propfind_folders(xml: &str, base_url: &str) -> Vec<String> {
    let mut folders = Vec::new();
    for block in xml.split("<D:response>").skip(1) {
        if !block.contains("<D:collection") {
            continue;
        }
        if let Some(start) = block.find("<D:href>") {
            let rest = &block[start + 8..];
            if let Some(end) = rest.find("</D:href>") {
                let href = rest[..end].trim().to_string();
                if href.trim_end_matches('/') != base_url.trim_end_matches('/') {
                    folders.push(href);
                }
            }
        }
    }
    folders
}
