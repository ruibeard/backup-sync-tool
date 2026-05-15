use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct PairStartRequest {
    pub machine_name: String,
    pub app_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairStartResponse {
    pub code: String,
    pub approve_url: String,
    pub poll_token: String,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairStatusResponse {
    pub status: String,
    pub device_token: Option<String>,
    pub webdav_url: Option<String>,
    pub username: Option<String>,
    pub remote_folder: Option<String>,
}

pub fn start_pairing(api_base: &str, machine_name: &str, app_version: &str) -> Option<PairStartResponse> {
    let req = PairStartRequest {
        machine_name: machine_name.to_string(),
        app_version: app_version.to_string(),
    };
    let url = format!("{}/api/pair/start", api_base.trim_end_matches('/'));
    let res = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(&serde_json::to_string(&req).ok()?)
        .ok()?;
    let body = res.into_string().ok()?;
    serde_json::from_str(&body).ok()
}

pub fn poll_pairing(api_base: &str, poll_token: &str) -> Option<PairStatusResponse> {
    let url = format!(
        "{}/api/pair/status/{}",
        api_base.trim_end_matches('/'),
        poll_token
    );
    let res = ureq::get(&url).call().ok()?;
    let body = res.into_string().ok()?;
    serde_json::from_str(&body).ok()
}
