use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct PairStartRequest {
    pub machine_name: String,
    pub windows_user: String,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_folder: Option<String>,
    pub supported_transports: Vec<String>,
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
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub webdav_url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub remote_folder: Option<String>,
    pub credential_profile_id: Option<u64>,
    pub credential_version: Option<u64>,
    #[serde(default)]
    pub s3_endpoint: Option<String>,
    #[serde(default)]
    pub s3_region: Option<String>,
    #[serde(default)]
    pub s3_bucket: Option<String>,
    #[serde(default)]
    pub s3_access_key: Option<String>,
    #[serde(default)]
    pub s3_secret_key: Option<String>,
    #[serde(default)]
    pub s3_path_style: Option<bool>,
    #[serde(default)]
    pub s3_prefix: Option<String>,
}

pub fn start_pairing(
    api_base: &str,
    machine_name: &str,
    windows_user: &str,
    app_version: &str,
    detected_folder: Option<String>,
) -> Option<PairStartResponse> {
    let req = PairStartRequest {
        machine_name: machine_name.to_string(),
        windows_user: windows_user.to_string(),
        app_version: app_version.to_string(),
        detected_folder,
        supported_transports: vec!["s3".to_string()],
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

pub fn is_s3_approval(status: &PairStatusResponse) -> bool {
    if let Some(transport) = status.transport.as_deref() {
        return transport.eq_ignore_ascii_case("s3");
    }
    status
        .s3_endpoint
        .as_ref()
        .is_some_and(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_start_serializes_supported_transports() {
        let req = PairStartRequest {
            machine_name: "PC".into(),
            windows_user: "u".into(),
            app_version: "2026.0.7".into(),
            detected_folder: None,
            supported_transports: vec!["s3".into()],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["supported_transports"], serde_json::json!(["s3"]));
    }

    #[test]
    fn s3_approval_prefers_transport_field() {
        let s3 = PairStatusResponse {
            status: "approved".into(),
            device_token: Some("t".into()),
            transport: Some("s3".into()),
            webdav_url: None,
            username: None,
            password: None,
            remote_folder: Some("Cust".into()),
            credential_profile_id: None,
            credential_version: None,
            s3_endpoint: Some("https://s3.rui.cam".into()),
            s3_region: None,
            s3_bucket: Some("device-1".into()),
            s3_access_key: None,
            s3_secret_key: None,
            s3_path_style: None,
            s3_prefix: Some(String::new()),
        };
        assert!(is_s3_approval(&s3));

        let webdav = PairStatusResponse {
            transport: None,
            s3_endpoint: None,
            s3_bucket: None,
            ..s3
        };
        assert!(!is_s3_approval(&webdav));
    }
}
