// S3-compatible transport: SigV4 PutObject + persistent multipart (Phase 2).

mod multipart;

use crate::config::Config;
use crate::transport::download::{apply_mtime, stream_to_atomic_file};
use crate::transport::{BackupTransport, FileMetadata, ObjectHead, RemoteFile, TransportError};
use multipart::{
    build_complete_xml, choose_part_size, complete_response_error, decide_after_lost_complete,
    decide_verified_receipt, delete_state, ensure_state_dir, expected_part_size, is_no_such_upload,
    is_transient, load_state, multipart_state_dir, new_client_upload_token, next_list_parts_marker,
    normalize_etag, parse_list_parts, parse_upload_id, part_count, part_offset, read_part_buffer,
    reconcile_parts, retained_part_digest_ok, save_state_atomic, sha256_hex, sleep_backoff,
    source_changed, state_path_for_identity, storage_identity, CompletedPart, IdentityUploadGuard,
    LostCompleteDecision, MultipartPhase, MultipartState, ObjectVerifyHead, ServerPart,
    VerifiedReceiptDecision, META_UPLOAD_TOKEN, RETRY_ATTEMPTS, STATE_VERSION,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

const EMPTY_PAYLOAD_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const META_MTIME: &str = "x-amz-meta-backup-mtime";

pub struct S3Transport {
    endpoint: String,
    bucket: String,
    request_bucket: Bucket,
    credentials: Credentials,
    prefix: String,
    /// PutObject threshold (= configured part size, 16–64 MiB).
    small_file_limit: u64,
    configured_part_mib: u64,
    control_agent: ureq::Agent,
    transfer_agent: ureq::Agent,
}

impl S3Transport {
    pub fn new(cfg: &Config, secret_key: &str) -> Result<Self, String> {
        let endpoint = cfg.s3_endpoint.trim().trim_end_matches('/').to_string();
        if !endpoint.to_ascii_lowercase().starts_with("https://") {
            return Err("S3 endpoint must use https://".into());
        }
        let region = if cfg.s3_region.trim().is_empty() {
            "us-east-1".to_string()
        } else {
            cfg.s3_region.trim().to_string()
        };
        let part_mib = cfg.s3_part_size_mib.clamp(16, 64);
        let request_bucket = Bucket::new(
            endpoint
                .parse()
                .map_err(|error| format!("Invalid S3 endpoint: {error}"))?,
            if cfg.s3_path_style {
                UrlStyle::Path
            } else {
                UrlStyle::VirtualHost
            },
            cfg.s3_bucket.trim().to_string(),
            region.clone(),
        )
        .map_err(|error| format!("Invalid S3 bucket configuration: {error}"))?;
        Ok(Self {
            endpoint,
            bucket: cfg.s3_bucket.trim().to_string(),
            request_bucket,
            credentials: Credentials::new(cfg.s3_access_key.trim(), secret_key),
            prefix: cfg.s3_prefix.trim().trim_matches('/').to_string(),
            small_file_limit: part_mib * 1024 * 1024,
            configured_part_mib: part_mib,
            control_agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(30))
                .build(),
            transfer_agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(30))
                .timeout_read(Duration::from_secs(600))
                .timeout_write(Duration::from_secs(600))
                .build(),
        })
    }

    fn object_key(&self, relative_path: &str) -> String {
        let relative = relative_path.trim_start_matches('/').replace('\\', "/");
        if self.prefix.is_empty() {
            relative
        } else {
            format!("{}/{}", self.prefix, relative)
        }
    }

    fn sign_action<'a, A: S3Action<'a>>(
        &self,
        mut action: A,
        query: &[(String, String)],
        headers: &[(&str, String)],
    ) -> String {
        for (name, value) in query {
            if matches!(
                name.as_str(),
                "partNumber" | "uploadId" | "uploads" | "list-type"
            ) {
                continue;
            }
            action.query_mut().insert(name.clone(), value.clone());
        }
        for (name, value) in headers {
            action
                .headers_mut()
                .insert(name.to_ascii_lowercase(), value.clone());
        }
        action.sign(Duration::from_secs(15 * 60)).to_string()
    }

    fn signed_url(
        &self,
        method: &str,
        key: &str,
        query: &[(String, String)],
        headers: &[(&str, String)],
    ) -> Result<String, TransportError> {
        let value = |name: &str| {
            query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let credentials = Some(&self.credentials);
        let url = match method {
            "PUT" if value("partNumber").is_some() => {
                let part_number = value("partNumber")
                    .and_then(|number| number.parse::<u16>().ok())
                    .ok_or_else(|| TransportError::Other("Invalid multipart part number".into()))?;
                let upload_id = value("uploadId")
                    .ok_or_else(|| TransportError::Other("Missing multipart upload ID".into()))?;
                self.sign_action(
                    self.request_bucket
                        .upload_part(credentials, key, part_number, upload_id),
                    query,
                    headers,
                )
            }
            "PUT" => self.sign_action(
                self.request_bucket.put_object(credentials, key),
                query,
                headers,
            ),
            "GET" if key.is_empty() => self.sign_action(
                self.request_bucket.list_objects_v2(credentials),
                query,
                headers,
            ),
            "GET" if value("uploadId").is_some() => self.sign_action(
                self.request_bucket.list_parts(
                    credentials,
                    key,
                    value("uploadId").unwrap_or_default(),
                ),
                query,
                headers,
            ),
            "GET" => self.sign_action(
                self.request_bucket.get_object(credentials, key),
                query,
                headers,
            ),
            "HEAD" if key.is_empty() => {
                self.sign_action(self.request_bucket.head_bucket(credentials), query, headers)
            }
            "HEAD" => self.sign_action(
                self.request_bucket.head_object(credentials, key),
                query,
                headers,
            ),
            "DELETE" if value("uploadId").is_some() => self.sign_action(
                self.request_bucket.abort_multipart_upload(
                    credentials,
                    key,
                    value("uploadId").unwrap_or_default(),
                ),
                query,
                headers,
            ),
            "DELETE" => self.sign_action(
                self.request_bucket.delete_object(credentials, key),
                query,
                headers,
            ),
            "POST" if value("uploads").is_some() => self.sign_action(
                self.request_bucket
                    .create_multipart_upload(credentials, key),
                query,
                headers,
            ),
            "POST" if value("uploadId").is_some() => self.sign_action(
                self.request_bucket.complete_multipart_upload(
                    credentials,
                    key,
                    value("uploadId").unwrap_or_default(),
                    std::iter::empty::<&str>(),
                ),
                query,
                headers,
            ),
            _ => {
                return Err(TransportError::Other(format!(
                    "Unsupported S3 request: {method}"
                )))
            }
        };
        Ok(url)
    }

    fn request(
        &self,
        agent: &ureq::Agent,
        method: &str,
        key: &str,
        query: &[(String, String)],
        extra_headers: &[(&str, String)],
        body: Option<&[u8]>,
        _payload_hash: &str,
    ) -> Result<ureq::Response, TransportError> {
        let url = self.signed_url(method, key, query, extra_headers)?;

        let mut req = agent.request(method, &url);
        for (name, value) in extra_headers {
            req = req.set(name, value);
        }

        let response = match body {
            Some(bytes) => req.send_bytes(bytes),
            None => req.call(),
        };

        match response {
            Ok(resp) => Ok(resp),
            Err(ureq::Error::Status(status, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(classify_s3_error(status, &body))
            }
            Err(err) => Err(TransportError::Other(err.to_string())),
        }
    }

    fn put_object_small(
        &self,
        relative_path: &str,
        local_path: &Path,
        metadata: &FileMetadata,
        size: u64,
    ) -> Result<(), TransportError> {
        let file = fs::File::open(local_path).map_err(|e| TransportError::Other(e.to_string()))?;
        let key = self.object_key(relative_path);
        let mut extra_headers: Vec<(&str, String)> = vec![("content-length", size.to_string())];
        if metadata.mtime > 0 {
            extra_headers.push((META_MTIME, metadata.mtime.to_string()));
        }

        let url = self.signed_url("PUT", &key, &[], &extra_headers)?;
        let mut request = self.transfer_agent.put(&url);
        for (name, value) in &extra_headers {
            request = request.set(name, value);
        }
        let resp = match request.send(file) {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let body = response.into_string().unwrap_or_default();
                return Err(classify_s3_error(status, &body));
            }
            Err(error) => return Err(TransportError::Other(error.to_string())),
        };
        if resp.status() >= 400 {
            return Err(TransportError::Http(resp.status(), "PutObject".into()));
        }

        match self.head_file(relative_path)? {
            Some(head) if head.size == size => Ok(()),
            Some(head) => Err(TransportError::Other(format!(
                "HeadObject size mismatch after PutObject: got {}, expected {size}",
                head.size
            ))),
            None => Err(TransportError::Other(
                "HeadObject missing after PutObject".into(),
            )),
        }
    }

    fn upload_multipart(
        &self,
        relative_path: &str,
        local_path: &Path,
        _metadata: &FileMetadata,
        _initial_size: u64,
    ) -> Result<(), TransportError> {
        let key = self.object_key(relative_path);
        let identity = storage_identity(&self.endpoint, &self.bucket, &key);
        let state_dir = multipart_state_dir();
        ensure_state_dir(&state_dir)?;
        let state_path = state_path_for_identity(&state_dir, &identity);
        let local_path_str = local_path.to_string_lossy().to_string();

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            if attempts > 3 {
                return Err(TransportError::Other(
                    "Multipart upload failed after restart attempts".into(),
                ));
            }

            let (size, mtime_ns) = stat_source(local_path)?;
            let header_mtime = mtime_ns / 1_000_000_000;

            match self.multipart_once(
                &key,
                local_path,
                &local_path_str,
                size,
                mtime_ns,
                header_mtime,
                &state_path,
            ) {
                Ok(()) => return Ok(()),
                Err(TransportError::SourceChanged) => return Err(TransportError::SourceChanged),
                Err(err) if is_restartable_multipart(&err) && attempts < 3 => {
                    let _ = self.abort_and_clear_state(&key, None, &state_path);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn multipart_once(
        &self,
        key: &str,
        local_path: &Path,
        local_path_str: &str,
        size: u64,
        mtime_ns: u64,
        header_mtime: u64,
        state_path: &Path,
    ) -> Result<(), TransportError> {
        let part_size = choose_part_size(size, self.configured_part_mib)?;
        let mut state = match self.load_or_create_state(
            key,
            local_path_str,
            size,
            mtime_ns,
            header_mtime,
            part_size,
            state_path,
        )? {
            LoadOrCreate::VerifiedReuse => return Ok(()),
            LoadOrCreate::State(s) => s,
        };

        // Active MPU only — Verified receipts never reach ListParts.
        match self.reconcile_state_with_server(&mut state, state_path) {
            Ok(()) => {}
            Err(err) if is_no_such_upload(&err) => {
                return self.recover_no_such_upload(&state, state_path, size);
            }
            Err(err) => return Err(err),
        }

        let total = part_count(state.local_size, state.part_size);

        for part_number in 1..=total {
            if let Some(idx) = state
                .completed_parts
                .iter()
                .position(|p| p.number == part_number)
            {
                let retained = state.completed_parts[idx].clone();
                let expected = expected_part_size(part_number, state.local_size, state.part_size);
                let offset = part_offset(part_number, state.part_size);
                let buf = read_part_buffer(local_path, offset, expected)?;
                let digest = sha256_hex(&buf);
                if retained_part_digest_ok(&retained, state.local_size, state.part_size, &digest) {
                    continue;
                }
                // Same-size content change (or missing digest) — drop and reupload.
                state.completed_parts.remove(idx);
                save_state_atomic(state_path, &state)?;
            }

            let (cur_size, cur_mtime_ns) = stat_source(local_path)?;
            if source_changed(&state, cur_size, cur_mtime_ns) {
                let _ = self.abort_multipart(key, &state.upload_id);
                delete_state(state_path)?;
                return Err(TransportError::SourceChanged);
            }

            let expected = expected_part_size(part_number, state.local_size, state.part_size);
            let offset = part_offset(part_number, state.part_size);
            let buf = read_part_buffer(local_path, offset, expected)?;
            let digest = sha256_hex(&buf);
            let (cur_size, cur_mtime_ns) = stat_source(local_path)?;
            if source_changed(&state, cur_size, cur_mtime_ns) {
                let _ = self.abort_multipart(key, &state.upload_id);
                delete_state(state_path)?;
                return Err(TransportError::SourceChanged);
            }
            let etag = match self.upload_part_with_retries(key, &state.upload_id, part_number, &buf)
            {
                Ok(etag) => etag,
                Err(err) if is_no_such_upload(&err) => {
                    return self.recover_no_such_upload(&state, state_path, state.local_size);
                }
                Err(err) => return Err(err),
            };

            state.completed_parts.push(CompletedPart {
                number: part_number,
                etag,
                size: expected,
                sha256: digest,
            });
            state.completed_parts.sort_by_key(|p| p.number);
            state.phase = MultipartPhase::Uploading;
            save_state_atomic(state_path, &state)?;
        }

        let (cur_size, cur_mtime_ns) = stat_source(local_path)?;
        if source_changed(&state, cur_size, cur_mtime_ns) {
            let _ = self.abort_multipart(key, &state.upload_id);
            delete_state(state_path)?;
            return Err(TransportError::SourceChanged);
        }

        state.phase = MultipartPhase::Completing;
        save_state_atomic(state_path, &state)?;

        match self.complete_multipart_with_retries(key, &state) {
            Ok(()) => {}
            Err(err) if is_no_such_upload(&err) => {
                return self.recover_no_such_upload(&state, state_path, state.local_size);
            }
            Err(err) if is_ambiguous_complete(&err) => {
                if self.verify_upload_token(key, state.local_size, &state.client_upload_token)? {
                    return self.finish_verified(&mut state, local_path, state_path);
                }
                // Keep Completing state for an identical retry / ListParts reconcile.
                return Err(err);
            }
            Err(err) => return Err(err),
        }

        if !self.verify_upload_token(key, state.local_size, &state.client_upload_token)? {
            return Err(TransportError::Other(
                "HeadObject size/token mismatch after CompleteMultipartUpload".into(),
            ));
        }

        self.finish_verified(&mut state, local_path, state_path)
    }

    fn finish_verified(
        &self,
        state: &mut MultipartState,
        local_path: &Path,
        state_path: &Path,
    ) -> Result<(), TransportError> {
        state.phase = MultipartPhase::Verified;
        save_state_atomic(state_path, state)?;

        let (cur_size, cur_mtime_ns) = stat_source(local_path)?;
        if source_changed(state, cur_size, cur_mtime_ns) {
            // Object is verified for the uploaded source identity; keep the receipt
            // so an unchanged re-attempt can short-circuit. Do not update manifest.
            return Err(TransportError::SourceChanged);
        }
        Ok(())
    }

    fn load_or_create_state(
        &self,
        key: &str,
        local_path_str: &str,
        size: u64,
        mtime_ns: u64,
        header_mtime: u64,
        part_size: u64,
        state_path: &Path,
    ) -> Result<LoadOrCreate, TransportError> {
        if let Some(existing) = load_state(state_path)? {
            let endpoint_norm = self.endpoint.trim_end_matches('/');
            let identity_ok = existing.endpoint.trim_end_matches('/') == endpoint_norm
                && existing.bucket == self.bucket
                && existing.object_key == key;

            if existing.phase == MultipartPhase::Verified && identity_ok {
                let changed = existing.local_path != local_path_str
                    || source_changed(&existing, size, mtime_ns);
                let head = self.head_object_verify(key)?;
                match decide_verified_receipt(
                    changed,
                    head.as_ref(),
                    existing.local_size,
                    &existing.client_upload_token,
                ) {
                    VerifiedReceiptDecision::ReuseSuccess => {
                        return Ok(LoadOrCreate::VerifiedReuse);
                    }
                    VerifiedReceiptDecision::ClearAndRestart => {
                        // Completed object — never AbortMultipartUpload a verified receipt.
                        delete_state(state_path)?;
                    }
                }
            } else if identity_ok
                && existing.local_path == local_path_str
                && !source_changed(&existing, size, mtime_ns)
                && existing.part_size > 0
                && !existing.upload_id.is_empty()
                && !existing.client_upload_token.is_empty()
                && existing.phase != MultipartPhase::Verified
            {
                return Ok(LoadOrCreate::State(existing));
            } else {
                // Stale active MPU — abort if possible, then recreate.
                if existing.phase != MultipartPhase::Verified && !existing.upload_id.is_empty() {
                    let _ = self.abort_multipart(key, &existing.upload_id);
                }
                delete_state(state_path)?;
            }
        }

        let token = new_client_upload_token();
        let upload_id = self.create_multipart(key, header_mtime, &token)?;
        let state = MultipartState {
            version: STATE_VERSION,
            endpoint: self.endpoint.trim_end_matches('/').to_string(),
            bucket: self.bucket.clone(),
            object_key: key.to_string(),
            local_path: local_path_str.to_string(),
            local_size: size,
            local_mtime_ns: mtime_ns,
            part_size,
            upload_id,
            client_upload_token: token,
            completed_parts: Vec::new(),
            phase: MultipartPhase::Uploading,
        };
        save_state_atomic(state_path, &state)?;
        Ok(LoadOrCreate::State(state))
    }

    fn reconcile_state_with_server(
        &self,
        state: &mut MultipartState,
        state_path: &Path,
    ) -> Result<(), TransportError> {
        let server = self.list_all_parts(&state.object_key, &state.upload_id)?;
        let result = reconcile_parts(
            &state.completed_parts,
            &server,
            state.local_size,
            state.part_size,
        );
        state.completed_parts = result.parts;
        save_state_atomic(state_path, state)?;
        Ok(())
    }

    fn recover_no_such_upload(
        &self,
        state: &MultipartState,
        state_path: &Path,
        expected_size: u64,
    ) -> Result<(), TransportError> {
        let head = self.head_object_verify(&state.object_key)?;
        match decide_after_lost_complete(head.as_ref(), expected_size, &state.client_upload_token) {
            LostCompleteDecision::Success => {
                let mut verified = state.clone();
                self.finish_verified(&mut verified, Path::new(&state.local_path), state_path)
            }
            LostCompleteDecision::Restart => {
                delete_state(state_path)?;
                Err(TransportError::Other(
                    "NoSuchUpload; final object not verified — restart multipart".into(),
                ))
            }
        }
    }

    fn abort_and_clear_state(
        &self,
        key: &str,
        upload_id: Option<&str>,
        state_path: &Path,
    ) -> Result<(), TransportError> {
        if let Ok(Some(state)) = load_state(state_path) {
            if state.phase != MultipartPhase::Verified {
                let id = upload_id.unwrap_or(state.upload_id.as_str());
                if !id.is_empty() {
                    let _ = self.abort_multipart(key, id);
                }
            }
        } else if let Some(id) = upload_id {
            let _ = self.abort_multipart(key, id);
        }
        delete_state(state_path)
    }

    fn create_multipart(
        &self,
        key: &str,
        mtime: u64,
        upload_token: &str,
    ) -> Result<String, TransportError> {
        let mut extra: Vec<(&str, String)> = Vec::new();
        if mtime > 0 {
            extra.push((META_MTIME, mtime.to_string()));
        }
        extra.push((META_UPLOAD_TOKEN, upload_token.to_string()));
        let query = vec![("uploads".into(), String::new())];
        // A lost Create response is ambiguous and cannot be safely retried without
        // discovering the orphaned UploadId, so issue it once.
        let resp = self.request(
            &self.control_agent,
            "POST",
            key,
            &query,
            &extra,
            None,
            EMPTY_PAYLOAD_HASH,
        )?;
        if resp.status() >= 400 {
            return Err(TransportError::Http(
                resp.status(),
                "CreateMultipartUpload".into(),
            ));
        }
        let xml = resp
            .into_string()
            .map_err(|e| TransportError::Other(e.to_string()))?;
        parse_upload_id(&xml)
    }

    fn upload_part_with_retries(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u32,
        body: &[u8],
    ) -> Result<String, TransportError> {
        let mut last_err = None;
        for attempt in 0..RETRY_ATTEMPTS {
            match self.upload_part(key, upload_id, part_number, body) {
                Ok(etag) => return Ok(etag),
                Err(err) if err.is_auth_failed() || is_no_such_upload(&err) => return Err(err),
                Err(err) if is_transient(&err) && attempt + 1 < RETRY_ATTEMPTS => {
                    last_err = Some(err);
                    sleep_backoff(attempt);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| TransportError::Other("UploadPart failed".into())))
    }

    fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u32,
        body: &[u8],
    ) -> Result<String, TransportError> {
        let payload_hash = hex::encode(Sha256::digest(body));
        let query = vec![
            ("partNumber".into(), part_number.to_string()),
            ("uploadId".into(), upload_id.to_string()),
        ];
        let extra = vec![("content-length", body.len().to_string())];
        let resp = self.request(
            &self.transfer_agent,
            "PUT",
            key,
            &query,
            &extra,
            Some(body),
            &payload_hash,
        )?;
        if resp.status() >= 400 {
            return Err(TransportError::Http(resp.status(), "UploadPart".into()));
        }
        let etag = resp
            .header("etag")
            .or_else(|| resp.header("ETag"))
            .map(normalize_etag)
            .filter(|e| !e.is_empty() && e != "\"\"")
            .ok_or_else(|| TransportError::Other("UploadPart response missing ETag".into()))?;
        Ok(etag)
    }

    fn list_all_parts(
        &self,
        key: &str,
        upload_id: &str,
    ) -> Result<Vec<ServerPart>, TransportError> {
        let mut all = Vec::new();
        let mut marker: Option<u32> = None;
        let mut pages_seen = 0u32;
        loop {
            pages_seen += 1;
            let mut query = vec![
                ("uploadId".into(), upload_id.to_string()),
                ("max-parts".into(), "1000".into()),
            ];
            if let Some(m) = marker {
                query.push(("part-number-marker".into(), m.to_string()));
            }
            let resp = self.retry_control(|| {
                self.request(
                    &self.control_agent,
                    "GET",
                    key,
                    &query,
                    &[],
                    None,
                    EMPTY_PAYLOAD_HASH,
                )
            })?;
            if resp.status() >= 400 {
                let status = resp.status();
                let body = resp.into_string().unwrap_or_default();
                return Err(classify_s3_error(status, &body));
            }
            let xml = resp
                .into_string()
                .map_err(|e| TransportError::Other(e.to_string()))?;
            let page = parse_list_parts(&xml)?;
            all.extend(page.parts.iter().cloned());
            match next_list_parts_marker(marker, &page, pages_seen)? {
                Some(next) => marker = Some(next),
                None => break,
            }
        }
        Ok(all)
    }

    fn complete_multipart_with_retries(
        &self,
        key: &str,
        state: &MultipartState,
    ) -> Result<(), TransportError> {
        let mut last_err = None;
        for attempt in 0..RETRY_ATTEMPTS {
            match self.complete_multipart(key, &state.upload_id, &state.completed_parts) {
                Ok(()) => return Ok(()),
                Err(err) if err.is_auth_failed() || is_no_such_upload(&err) => return Err(err),
                Err(err) if is_ambiguous_complete(&err) => return Err(err),
                Err(err) if is_transient(&err) && attempt + 1 < RETRY_ATTEMPTS => {
                    last_err = Some(err);
                    sleep_backoff(attempt);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err
            .unwrap_or_else(|| TransportError::Other("CompleteMultipartUpload failed".into())))
    }

    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[CompletedPart],
    ) -> Result<(), TransportError> {
        let body = build_complete_xml(parts);
        let payload_hash = hex::encode(Sha256::digest(body.as_bytes()));
        let query = vec![("uploadId".into(), upload_id.to_string())];
        let extra = vec![
            ("content-type", "application/xml".into()),
            ("content-length", body.len().to_string()),
        ];
        let resp = self.request(
            &self.transfer_agent,
            "POST",
            key,
            &query,
            &extra,
            Some(body.as_bytes()),
            &payload_hash,
        );
        match resp {
            Ok(resp) => {
                let status = resp.status();
                let xml = resp.into_string().unwrap_or_default();
                if status >= 400 {
                    return Err(classify_s3_error(status, &xml));
                }
                if let Some(code) = complete_response_error(&xml) {
                    return Err(TransportError::Http(status, format!("S3:{code}")));
                }
                Ok(())
            }
            Err(err) => {
                // Semantic S3 / auth errors are definitive. Other transport failures after
                // Complete may mean the object was committed — treat as ambiguous.
                if matches!(
                    &err,
                    TransportError::Http(..)
                        | TransportError::AuthFailed(_)
                        | TransportError::NotFound
                        | TransportError::TooLarge { .. }
                        | TransportError::SourceChanged
                ) || is_no_such_upload(&err)
                    || is_transient(&err)
                {
                    Err(err)
                } else {
                    Err(TransportError::Other(format!(
                        "CompleteMultipartUpload ambiguous: {err}"
                    )))
                }
            }
        }
    }

    fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<(), TransportError> {
        let query = vec![("uploadId".into(), upload_id.to_string())];
        match self.request(
            &self.control_agent,
            "DELETE",
            key,
            &query,
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        ) {
            Ok(resp) if resp.status() < 400 || resp.status() == 404 => Ok(()),
            Ok(resp) => Err(TransportError::Http(
                resp.status(),
                "AbortMultipartUpload".into(),
            )),
            Err(TransportError::NotFound) => Ok(()),
            Err(err) if is_no_such_upload(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn head_object_verify(&self, key: &str) -> Result<Option<ObjectVerifyHead>, TransportError> {
        match self.request(
            &self.control_agent,
            "HEAD",
            key,
            &[],
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        ) {
            Ok(resp) => {
                if resp.status() == 404 {
                    return Ok(None);
                }
                if resp.status() >= 400 {
                    return Err(TransportError::Http(resp.status(), "HeadObject".into()));
                }
                let size = resp
                    .header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let upload_token = resp
                    .header(META_UPLOAD_TOKEN)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty());
                Ok(Some(ObjectVerifyHead { size, upload_token }))
            }
            Err(TransportError::NotFound) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn verify_upload_token(
        &self,
        key: &str,
        expected_size: u64,
        expected_token: &str,
    ) -> Result<bool, TransportError> {
        let head = self.head_object_verify(key)?;
        Ok(
            decide_after_lost_complete(head.as_ref(), expected_size, expected_token)
                == LostCompleteDecision::Success,
        )
    }

    fn retry_control<F>(&self, mut f: F) -> Result<ureq::Response, TransportError>
    where
        F: FnMut() -> Result<ureq::Response, TransportError>,
    {
        let mut last_err = None;
        for attempt in 0..RETRY_ATTEMPTS {
            match f() {
                Ok(resp) => return Ok(resp),
                Err(err) if err.is_auth_failed() || is_no_such_upload(&err) => return Err(err),
                Err(err) if is_transient(&err) && attempt + 1 < RETRY_ATTEMPTS => {
                    last_err = Some(err);
                    sleep_backoff(attempt);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| TransportError::Other("S3 control request failed".into())))
    }
}

impl BackupTransport for S3Transport {
    fn test_connection(&self) -> Result<(), TransportError> {
        // List with max-keys=1 (optional device prefix when configured).
        let mut query = vec![
            ("list-type".into(), "2".into()),
            ("max-keys".into(), "1".into()),
        ];
        if !self.prefix.is_empty() {
            query.push(("prefix".into(), format!("{}/", self.prefix)));
        }
        let resp = self.request(
            &self.control_agent,
            "GET",
            "",
            &query,
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        )?;
        if resp.status() < 400 {
            Ok(())
        } else {
            Err(TransportError::Http(resp.status(), "ListObjectsV2".into()))
        }
    }

    fn upload_file(
        &self,
        relative_path: &str,
        local_path: &Path,
        metadata: &FileMetadata,
    ) -> Result<(), TransportError> {
        let key = self.object_key(relative_path);
        let identity = storage_identity(&self.endpoint, &self.bucket, &key);
        let _identity_lock = IdentityUploadGuard::acquire(&identity);
        let meta = fs::metadata(local_path).map_err(|e| TransportError::Other(e.to_string()))?;
        let size = meta.len();
        if size <= self.small_file_limit {
            self.put_object_small(relative_path, local_path, metadata, size)
        } else {
            self.upload_multipart(relative_path, local_path, metadata, size)
        }
    }

    fn download_file(
        &self,
        relative_path: &str,
        destination_path: &Path,
    ) -> Result<FileMetadata, TransportError> {
        let key = self.object_key(relative_path);
        let head = self.head_file(relative_path)?;
        let resp = self.request(
            &self.transfer_agent,
            "GET",
            &key,
            &[],
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        )?;
        if resp.status() >= 400 {
            return Err(TransportError::Http(resp.status(), "GetObject".into()));
        }
        let expected = head.as_ref().map(|h| h.size);
        let size = stream_to_atomic_file(resp.into_reader(), destination_path, expected)?;
        let mtime = head.and_then(|h| h.mtime).unwrap_or(0);
        if mtime > 0 {
            let _ = apply_mtime(destination_path, mtime);
        }
        Ok(FileMetadata { size, mtime })
    }

    fn list_files(&self) -> Result<Vec<RemoteFile>, TransportError> {
        let mut files = Vec::new();
        let mut continuation: Option<String> = None;

        loop {
            let mut query = vec![("list-type".into(), "2".into())];
            if !self.prefix.is_empty() {
                query.push(("prefix".into(), format!("{}/", self.prefix)));
            }
            if let Some(token) = &continuation {
                query.push(("continuation-token".into(), token.clone()));
            }

            let resp = self.request(
                &self.control_agent,
                "GET",
                "",
                &query,
                &[],
                None,
                EMPTY_PAYLOAD_HASH,
            )?;
            if resp.status() >= 400 {
                return Err(TransportError::Http(resp.status(), "ListObjectsV2".into()));
            }
            let xml = resp
                .into_string()
                .map_err(|e| TransportError::Other(e.to_string()))?;
            let page = parse_list_objects_v2(&xml)?;
            for obj in page.objects {
                let Some(relative) = strip_prefix_key(&self.prefix, &obj.key) else {
                    continue;
                };
                if relative.is_empty() || relative.ends_with('/') {
                    continue;
                }
                files.push(RemoteFile {
                    relative_path: relative,
                    size: obj.size,
                    // ListObjectsV2 does not return custom metadata; mtime comes from remote manifest.
                    mtime: 0,
                });
            }
            if page.is_truncated {
                continuation = page.next_continuation_token;
            } else {
                break;
            }
        }

        Ok(files)
    }

    fn head_file(&self, relative_path: &str) -> Result<Option<ObjectHead>, TransportError> {
        let key = self.object_key(relative_path);
        match self.request(
            &self.control_agent,
            "HEAD",
            &key,
            &[],
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        ) {
            Ok(resp) => {
                if resp.status() == 404 {
                    return Ok(None);
                }
                if resp.status() >= 400 {
                    return Err(TransportError::Http(resp.status(), "HeadObject".into()));
                }
                let size = resp
                    .header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mtime = resp.header(META_MTIME).and_then(|v| v.parse::<u64>().ok());
                Ok(Some(ObjectHead { size, mtime }))
            }
            Err(TransportError::NotFound) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn delete_file(&self, relative_path: &str) -> Result<(), TransportError> {
        let key = self.object_key(relative_path);
        match self.request(
            &self.control_agent,
            "DELETE",
            &key,
            &[],
            &[],
            None,
            EMPTY_PAYLOAD_HASH,
        ) {
            Ok(resp) if resp.status() < 400 || resp.status() == 404 => Ok(()),
            Ok(resp) => Err(TransportError::Http(resp.status(), "DeleteObject".into())),
            Err(TransportError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[derive(Debug, Default)]
struct ListPage {
    objects: Vec<ListedObject>,
    is_truncated: bool,
    next_continuation_token: Option<String>,
}

#[derive(Debug, Default)]
struct ListedObject {
    key: String,
    size: u64,
}

fn parse_list_objects_v2(xml: &str) -> Result<ListPage, TransportError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut page = ListPage::default();
    let mut current: Option<ListedObject> = None;
    let mut text_target = String::new();
    let mut in_contents = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = local_name(e.name().as_ref());
                match tag.as_str() {
                    "Contents" => {
                        in_contents = true;
                        current = Some(ListedObject::default());
                    }
                    "Key" | "Size" | "IsTruncated" | "NextContinuationToken" => {
                        text_target = tag;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().map(|c| c.into_owned()).unwrap_or_default();
                match text_target.as_str() {
                    "Key" if in_contents => {
                        if let Some(obj) = current.as_mut() {
                            obj.key = text;
                        }
                    }
                    "Size" if in_contents => {
                        if let Some(obj) = current.as_mut() {
                            obj.size = text.parse().unwrap_or(0);
                        }
                    }
                    "IsTruncated" => {
                        page.is_truncated = text.eq_ignore_ascii_case("true");
                    }
                    "NextContinuationToken" => {
                        page.next_continuation_token = Some(text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_name(e.name().as_ref());
                if tag == "Contents" {
                    if let Some(obj) = current.take() {
                        if !obj.key.is_empty() {
                            page.objects.push(obj);
                        }
                    }
                    in_contents = false;
                }
                if tag == text_target {
                    text_target.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(TransportError::Other(format!(
                    "ListObjectsV2 XML parse error: {err}"
                )))
            }
            _ => {}
        }
    }
    Ok(page)
}

fn classify_s3_error(status: u16, body: &str) -> TransportError {
    let code = extract_s3_error_code(body).unwrap_or_default();
    if code == "NoSuchUpload" {
        return TransportError::Http(status, format!("S3:{code}"));
    }
    if status == 404 {
        return TransportError::NotFound;
    }
    let auth_codes = [
        "InvalidAccessKeyId",
        "SignatureDoesNotMatch",
        "ExpiredToken",
        "InvalidToken",
        "AccessDenied",
        "AccountProblem",
        "InvalidSecurity",
    ];
    if matches!(status, 401 | 403) && (code.is_empty() || auth_codes.iter().any(|c| code == *c)) {
        return TransportError::AuthFailed(if code.is_empty() {
            format!("S3 returned HTTP {status}")
        } else {
            format!("S3 auth/policy error: {code}")
        });
    }
    if !code.is_empty() {
        TransportError::Http(status, format!("S3:{code}"))
    } else {
        TransportError::Http(status, "S3".into())
    }
}

enum LoadOrCreate {
    VerifiedReuse,
    State(MultipartState),
}

fn stat_source(path: &Path) -> Result<(u64, u64), TransportError> {
    let meta = fs::metadata(path).map_err(|e| TransportError::Other(e.to_string()))?;
    Ok((meta.len(), file_mtime_ns(path)))
}

fn file_mtime_ns(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| {
            let nanos = d.as_nanos();
            u64::try_from(nanos).unwrap_or(u64::MAX)
        })
        .unwrap_or(0)
}

fn file_mtime_epoch(path: &Path) -> u64 {
    file_mtime_ns(path) / 1_000_000_000
}

fn is_restartable_multipart(err: &TransportError) -> bool {
    if err.is_source_changed() {
        return false;
    }
    match err {
        TransportError::Other(msg) => {
            msg.contains("restart multipart") || msg.contains("NoSuchUpload")
        }
        _ => is_no_such_upload(err),
    }
}

fn is_ambiguous_complete(err: &TransportError) -> bool {
    match err {
        TransportError::Other(msg) => msg.contains("ambiguous"),
        _ => false,
    }
}

fn extract_s3_error_code(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut in_code = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == "Code" => in_code = true,
            Ok(Event::Text(e)) if in_code => {
                return Some(e.unescape().map(|c| c.into_owned()).unwrap_or_default());
            }
            Ok(Event::End(e)) if local_name(e.name().as_ref()) == "Code" => in_code = false,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

fn strip_prefix_key(prefix: &str, key: &str) -> Option<String> {
    let prefix = prefix.trim_matches('/');
    let key = key.trim_start_matches('/');
    if prefix.is_empty() {
        return Some(key.to_string());
    }
    let with_slash = format!("{prefix}/");
    key.strip_prefix(&with_slash)
        .map(|s| s.to_string())
        .or_else(|| {
            if key == prefix {
                Some(String::new())
            } else {
                None
            }
        })
}

fn local_name(name: &[u8]) -> String {
    let name = std::str::from_utf8(name).unwrap_or_default();
    name.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_objects_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>cust/dev/folder/file.txt</Key>
    <Size>12</Size>
  </Contents>
  <Contents>
    <Key>cust/dev/.backupsynctool-remote-manifest.json</Key>
    <Size>2</Size>
  </Contents>
</ListBucketResult>"#;
        let page = parse_list_objects_v2(xml).unwrap();
        assert!(!page.is_truncated);
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].key, "cust/dev/folder/file.txt");
        assert_eq!(page.objects[0].size, 12);
    }

    #[test]
    fn classify_signature_mismatch_as_auth() {
        let body = r#"<?xml version="1.0"?><Error><Code>SignatureDoesNotMatch</Code><Message>x</Message></Error>"#;
        let err = classify_s3_error(403, body);
        assert!(err.is_auth_failed());
    }

    #[test]
    fn classify_404_as_not_found() {
        let err = classify_s3_error(404, "");
        assert!(matches!(err, TransportError::NotFound));
    }

    #[test]
    fn strip_device_prefix() {
        assert_eq!(
            strip_prefix_key("cust/dev", "cust/dev/a/b.txt").as_deref(),
            Some("a/b.txt")
        );
    }

    #[test]
    fn empty_prefix_keeps_object_relative_key() {
        assert_eq!(
            strip_prefix_key("", "report.zip").as_deref(),
            Some("report.zip")
        );
        assert_eq!(
            strip_prefix_key("", "dir/nested.txt").as_deref(),
            Some("dir/nested.txt")
        );
    }

    #[test]
    fn object_key_allows_empty_prefix() {
        let cfg = Config {
            s3_endpoint: "https://s3.rui.cam".into(),
            s3_bucket: "device-uuid".into(),
            s3_access_key: "AKIA".into(),
            s3_path_style: true,
            s3_prefix: String::new(),
            s3_part_size_mib: 32,
            ..Config::default()
        };
        let transport = S3Transport::new(&cfg, "secret").unwrap();
        assert_eq!(transport.object_key("a/b.txt"), "a/b.txt");
        assert_eq!(
            transport.object_key(".backupsynctool-remote-manifest.json"),
            ".backupsynctool-remote-manifest.json"
        );
    }

    #[test]
    fn too_large_rejects_beyond_multipart_capacity() {
        let err = TransportError::TooLarge {
            size: 5_000_000_000_000,
            limit: 5 * 1024 * 1024 * 1024 * 10_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("exceeds multipart capacity"));
        assert!(!err.is_auth_failed());
    }
}
