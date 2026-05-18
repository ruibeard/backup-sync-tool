// ── Utilities ─────────────────────────────────────────────────────────────────
unsafe fn set_status(hwnd: HWND, t: &str) {
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), &hstring(t));
}

unsafe fn set_status_dot_color(hwnd: HWND, color: u32) {
    stmut(hwnd).status_dot_color = color;
    InvalidateRect(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), None, TRUE);
}

unsafe fn restore_pair_idle_controls(hwnd: HWND) {
    let st = stmut(hwnd);
    let label = if st.auth_failure_notified {
        "Pair again"
    } else {
        "Pair"
    };
    let pair_hwnd = GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32);
    let _ = SetWindowTextW(pair_hwnd, &hstring(label));
    EnableWindow(pair_hwnd, TRUE);
    ShowWindow(GetDlgItem(hwnd, IDC_SAVE as i32), SW_SHOW);
}

unsafe fn restore_server_status_after_pair_cancel(hwnd: HWND) {
    let st = stmut(hwnd);
    let status = if st.auth_failure_notified {
        "Pair again required"
    } else if is_paired(&st.config) {
        "Paired"
    } else {
        "Pair cancelled"
    };
    let color = if st.auth_failure_notified || !st.connected {
        C_RED
    } else {
        C_GREEN
    };
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_SERVER_STATUS as i32), &hstring(status));
    set_status_dot_color(hwnd, color);
    set_status(hwnd, "\u{25cf}");
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
}

fn is_root_remote_folder(folder: &str) -> bool {
    let trimmed = folder.trim();
    trimmed.is_empty() || trimmed == "/" || trimmed == "\\"
}

fn is_paired(cfg: &Config) -> bool {
    !cfg.device_token_enc.trim().is_empty()
}

fn required_pair_field(value: Option<String>, name: &str) -> std::result::Result<String, String> {
    match value.and_then(non_empty) {
        Some(value) => Ok(value.trim().to_string()),
        None => Err(format!("Pairing approved but no {name} was returned.")),
    }
}

fn approved_remote_folder(remote_folder: Option<&str>) -> std::result::Result<String, String> {
    let Some(remote_folder) = remote_folder else {
        return Err("Pairing approved but no destination folder was returned.".to_string());
    };
    let raw = remote_folder.trim();
    if raw.is_empty() || raw == "/" || raw == "\\" {
        return Err(
            "Pairing approved without a customer destination folder. Re-pair after Laravel approves a concrete customer folder."
                .to_string(),
        );
    }
    if raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains("..")
        || raw.chars().any(char::is_control)
    {
        return Err(
            "Pairing approved with an invalid destination folder. Re-pair after Laravel approves a concrete customer folder."
                .to_string(),
        );
    }
    Ok(raw.to_string())
}

unsafe fn apply_server_readonly(hwnd: HWND) {
    update_server_tooltip(hwnd);
    let label = if is_paired(&stmut(hwnd).config) {
        "Approved folder"
    } else {
        "Destination folder"
    };
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_DEST_LABEL as i32), &hstring(label));
    stmut(hwnd).min_client_h = required_client_height(stmut(hwnd));
    layout_main(hwnd);
}

unsafe fn start_connection_check(hwnd: HWND) {
    let st = stmut(hwnd);
    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    if cfg.webdav_url.trim().is_empty() || cfg.username.trim().is_empty() || pass.trim().is_empty()
    {
        return;
    }
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let ok = webdav::test_connection(&cfg, &pass).is_ok();
        unsafe {
            PostMessageW(
                HWND(raw),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        }
    });
}

unsafe fn read_ctrls(hwnd: HWND, st: &mut WndState) {
    st.config.watch_folder = gettext(hwnd, IDC_WATCH_FOLDER);
    st.config.start_with_windows = checked(hwnd, IDC_START_WINDOWS);
    st.config.sync_remote_changes = checked(hwnd, IDC_SYNC_REMOTE);
}

unsafe fn gettext(hwnd: HWND, id: u16) -> String {
    let h = GetDlgItem(hwnd, id as i32);
    let n = GetWindowTextLengthW(h);
    if n == 0 {
        return String::new();
    }
    let mut b = vec![0u16; (n + 1) as usize];
    GetWindowTextW(h, &mut b);
    String::from_utf16_lossy(&b[..n as usize])
}

unsafe fn checked(hwnd: HWND, id: u16) -> bool {
    SendMessageW(
        GetDlgItem(hwnd, id as i32),
        BM_GETCHECK,
        WPARAM(0),
        LPARAM(0),
    )
    .0 == BST_CHECKED.0 as isize
}

unsafe fn stmut(hwnd: HWND) -> &'static mut WndState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState)
}
unsafe fn state_ptr(hwnd: HWND) -> *mut WndState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState
}

unsafe fn mkfont(name: &str, pt: i32, weight: i32) -> HFONT {
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
    ReleaseDC(None, hdc);
    let h = -(pt * dpi / 72);
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW::default();
    lf.lfHeight = h;
    lf.lfWeight = weight;
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

unsafe fn mkfont_underline(name: &str, pt: i32, weight: i32) -> HFONT {
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
    ReleaseDC(None, hdc);
    let h = -(pt * dpi / 72);
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW::default();
    lf.lfHeight = h;
    lf.lfWeight = weight;
    lf.lfUnderline = 1; // underline for clickable links
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

fn hstring(s: &str) -> HSTRING {
    HSTRING::from(s)
}
#[allow(dead_code)]
fn wstr(b: &[u16]) -> String {
    let e = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..e])
}

unsafe fn msgbox(hwnd: HWND, text: &str, title: &str) {
    MessageBoxW(
        hwnd,
        &hstring(text),
        &hstring(title),
        MB_OK | MB_ICONINFORMATION,
    );
}
unsafe fn msgbox_yn(hwnd: HWND, text: &str, title: &str) -> bool {
    MessageBoxW(
        hwnd,
        &hstring(text),
        &hstring(title),
        MB_YESNO | MB_ICONQUESTION,
    )
    .0 == IDYES.0 as i32
}

fn activity_entry(message: &str) -> Option<String> {
    if message.starts_with("Checking remote files") {
        return Some(message.to_string());
    }
    if message.starts_with("Counting local files") {
        return Some(message.to_string());
    }
    if message.starts_with("Comparing local to remote") {
        return Some(message.to_string());
    }
    if message.starts_with("Checking remote changes") {
        return Some(message.to_string());
    }
    if let Some(name) = message.strip_prefix("Uploading: ") {
        return Some(format!("Uploading {}", display_activity_name(name)));
    }
    if let Some(name) = message.strip_prefix("Uploaded: ") {
        return Some(format!("Uploaded {}", display_activity_name(name)));
    }
    if let Some(name) = message.strip_prefix("Downloaded: ") {
        return Some(format!("Downloaded {}", display_activity_name(name)));
    }
    None
}

fn display_activity_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn format_eta(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

fn validate_webdav_url(url: &str) -> std::result::Result<(), String> {
    if url.trim().to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err("Server URL must start with https://".to_string())
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

