// ── Utilities ─────────────────────────────────────────────────────────────────
unsafe fn set_status_dot_color(hwnd: HWND, color: u32) {
    let st = stmut(hwnd);
    st.status_dot_color = color;
    InvalidateRect(hwnd, Some(&st.status_strip_rect), TRUE);
}

unsafe fn invalidate_bridge(hwnd: HWND) {
    let st = stmut(hwnd);
    InvalidateRect(hwnd, Some(&st.bridge_rect), TRUE);
    if st.bridge_progress_rect.bottom > st.bridge_progress_rect.top {
        InvalidateRect(hwnd, Some(&st.bridge_progress_rect), TRUE);
    }
}

unsafe fn update_bridge_display(hwnd: HWND) {
    let st = stmut(hwnd);
    let paired = is_paired(&st.config);
    let auth_fail = st.auth_failure_notified;
    let is_checking = st.sync_status_state == UiSyncState::Checking as usize;
    let is_syncing = st.sync_status_state == UiSyncState::Syncing as usize;

    st.bridge_conn_ok = paired && st.connected && !auth_fail;
    st.bridge_conn_label = if !paired {
        "Not connected".to_string()
    } else if !st.config.syncthing_folder_label.trim().is_empty() {
        st.config.syncthing_folder_label.trim().to_string()
    } else if auth_fail {
        "Syncthing hub unavailable".to_string()
    } else {
        "Syncthing hub unavailable".to_string()
    };

    st.bridge_sync_head = if auth_fail && !is_syncing && !is_checking {
        "Sync paused".to_string()
    } else if !paired {
        "Not syncing".to_string()
    } else if is_syncing {
        "Syncing".to_string()
    } else if is_checking {
        "Checking".to_string()
    } else {
        "All synced".to_string()
    };

    st.bridge_sync_meta = if auth_fail && !is_syncing && !is_checking {
        "Assignment invalid\r\nreconnect to resume".to_string()
    } else if !paired {
        "Pair with server\r\nto start backup".to_string()
    } else if is_syncing {
        "Synchronizing\r\nchanges both ways".to_string()
    } else if is_checking {
        "Checking...".to_string()
    } else {
        "Synced".to_string()
    };

    invalidate_bridge(hwnd);

    let want_band = bridge_show_sync_band(st);
    let has_band = st.bridge_progress_rect.bottom > st.bridge_progress_rect.top;
    if want_band != has_band {
        layout_main(hwnd);
    }
}

unsafe fn restore_pair_idle_controls(hwnd: HWND) {
    let st = stmut(hwnd);
    let label = if st.auth_failure_notified || is_paired(&st.config) {
        "Reconnect Server"
    } else {
        "Connect Server"
    };
    let pair_hwnd = GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32);
    let _ = SetWindowTextW(pair_hwnd, &hstring(label));
    ShowWindow(pair_hwnd, SW_SHOW);
    update_pair_button_enabled(hwnd);
    layout_main(hwnd);
}

unsafe fn restore_server_status_after_pair_cancel(hwnd: HWND) {
    let st = stmut(hwnd);
    if st.auth_failure_notified {
        set_status_strip_text(hwnd, "Reconnect required");
        set_status_dot_color(hwnd, C_RED);
        return;
    }
    if is_paired(&st.config) {
        set_status_strip_connection(hwnd);
        return;
    }
    set_status_strip_text(hwnd, "Pair cancelled");
    set_status_dot_color(hwnd, C_RED);
}

fn sync_is_busy(st: &WndState) -> bool {
    st.sync_status_state == UiSyncState::Checking as usize
        || st.sync_status_state == UiSyncState::Syncing as usize
}

unsafe fn set_status_strip_text(hwnd: HWND, text: &str) {
    let st = stmut(hwnd);
    st.status_strip_display = text.to_string();
    if STATUS_ROW_H > 0 {
        InvalidateRect(hwnd, Some(&st.status_strip_rect), TRUE);
    }
    update_bridge_display(hwnd);
}

unsafe fn set_status_subtitle(hwnd: HWND, text: &str) {
    let st = stmut(hwnd);
    st.status_subtitle = text.to_string();
    if STATUS_ROW_H > 0 {
        InvalidateRect(hwnd, Some(&st.status_strip_rect), TRUE);
    }
}

unsafe fn set_status_strip_connection(hwnd: HWND) {
    let st = stmut(hwnd);
    if st.auth_failure_notified {
        update_bridge_display(hwnd);
        return;
    }
    let (text, color) = if !is_paired(&st.config) {
        if st.connected {
            ("Connected".to_string(), C_GREEN)
        } else {
            ("Not paired".to_string(), C_AMBER)
        }
    } else if st.connected {
        ("Connected".to_string(), C_GREEN)
    } else {
        ("Offline".to_string(), C_RED)
    };
    set_status_strip_text(hwnd, &text);
    set_status_dot_color(hwnd, color);
}

unsafe fn idle_sync_footer_label(st: &WndState) -> String {
    if is_paired(&st.config) {
        "All synced".to_string()
    } else {
        "Ready".to_string()
    }
}

unsafe fn update_sync_footer(hwnd: HWND, state: usize, progress: (usize, usize, usize)) {
    let status_hwnd = GetDlgItem(hwnd, IDC_SYNC_STATUS as i32);
    let eta_hwnd = GetDlgItem(hwnd, IDC_SYNC_ETA as i32);
    let prog_hwnd = GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32);
    let is_checking = state == UiSyncState::Checking as usize;
    let is_syncing = state == UiSyncState::Syncing as usize;
    let is_busy = is_checking || is_syncing;
    let st = stmut(hwnd);
    let was_busy = st.sync_footer_busy;
    st.sync_footer_busy = is_busy && (progress.1 > 0 || is_checking);
    let footer_h = 0;
    if st.sync_row_h != footer_h {
        st.sync_row_h = footer_h;
        layout_main(hwnd);
    } else if was_busy != st.sync_footer_busy {
        InvalidateRect(hwnd, Some(&st.sync_footer_rect), TRUE);
    }

    if is_busy && progress.1 > 0 {
        let done = progress.0.min(progress.1);
        let pct = (done * 100) / progress.1;
        let _ = pct;
        set_status_subtitle(hwnd, "");
    } else if is_busy {
        set_status_subtitle(hwnd, "");
    } else {
        let st = stmut(hwnd);
        let label = idle_sync_footer_label(st);
        set_status_subtitle(hwnd, &label);
        let _ = SetWindowTextW(status_hwnd, &hstring(&label));
        let _ = SetWindowTextW(eta_hwnd, &hstring(""));
    }

    ShowWindow(prog_hwnd, SW_HIDE);
    SendMessageW(prog_hwnd, PBM_SETPOS, WPARAM(0), LPARAM(0));

    update_bridge_display(hwnd);

    let footer_hwnd = GetDlgItem(hwnd, IDC_SYNC_STATUS as i32);
    ShowWindow(footer_hwnd, SW_HIDE);
    ShowWindow(GetDlgItem(hwnd, IDC_SYNC_ETA as i32), SW_HIDE);
}

unsafe fn update_status_strip_from_connection(hwnd: HWND) {
    let st = stmut(hwnd);
    if sync_is_busy(st) || st.auth_failure_notified {
        invalidate_bridge(hwnd);
        return;
    }
    set_status_strip_connection(hwnd);
    invalidate_bridge(hwnd);
    let progress = (st.sync_progress_done, st.sync_progress_total, 0);
    update_sync_footer(hwnd, st.sync_status_state, progress);
}

fn required_pair_field(value: Option<String>, name: &str) -> std::result::Result<String, String> {
    match value.and_then(non_empty) {
        Some(value) => Ok(value.trim().to_string()),
        None => Err(format!("Pairing approved but no {name} was returned.")),
    }
}

unsafe fn apply_server_readonly(hwnd: HWND) {
    update_server_tooltip(hwnd);
    let label = if is_paired(&stmut(hwnd).config) {
        "Server destination"
    } else {
        "Destination folder"
    };
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_DEST_LABEL as i32), &hstring(label));
    let st = stmut(hwnd);
    st.min_client_h = required_client_height(st);
    layout_main(hwnd);
    invalidate_bridge(hwnd);
}

unsafe fn start_connection_check(hwnd: HWND) {
    let connected = stmut(hwnd).app.snapshot().hub_connected;
    PostMessageW(
        hwnd,
        WM_APP_CONNECTED,
        WPARAM(usize::from(connected)),
        LPARAM(0),
    )
    .ok();
}

fn is_sync_configured(cfg: &Config) -> bool {
    watch_folder_is_valid(&cfg.watch_folder) && config::is_paired(cfg)
}

fn watch_folder_is_valid(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty() && Path::new(path).is_dir()
}

unsafe fn update_pair_button_enabled(hwnd: HWND) {
    let pair_hwnd = GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32);
    if pair_hwnd.0.is_null() {
        return;
    }
    let st = stmut(hwnd);
    let enabled = watch_folder_is_valid(&st.config.watch_folder);
    let _ = ShowWindow(pair_hwnd, if enabled { SW_SHOW } else { SW_HIDE });
    let _ = EnableWindow(pair_hwnd, enabled);
    let _ = InvalidateRect(pair_hwnd, None, TRUE);

    let refresh_hwnd = GetDlgItem(hwnd, IDC_REFRESH_REMOTE as i32);
    if !refresh_hwnd.0.is_null() {
        ShowWindow(refresh_hwnd, SW_HIDE);
    }
}

unsafe fn ensure_default_watch_folder(hwnd: HWND) {
    let st = stmut(hwnd);
    if watch_folder_is_valid(&st.config.watch_folder) {
        return;
    }
    if let Some(path) = crate::xd::default_watch_folder() {
        st.config.watch_folder = path;
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32),
            &hstring(&st.config.watch_folder),
        );
    }
}

unsafe fn ensure_or_prompt_watch_folder(hwnd: HWND) -> bool {
    {
        let st = stmut(hwnd);
        if !st.config.watch_folder.trim().is_empty() && Path::new(&st.config.watch_folder).is_dir()
        {
            return true;
        }
    }
    ensure_default_watch_folder(hwnd);
    {
        let st = stmut(hwnd);
        if !st.config.watch_folder.trim().is_empty() && Path::new(&st.config.watch_folder).is_dir()
        {
            return true;
        }
    }
    notify_user(
        hwnd,
        "Choose the backup folder on this PC. The XDSoftware backup folder was not found.",
    );
    if !browse_local(hwnd, false) {
        return false;
    }
    read_ctrls(hwnd, stmut(hwnd));
    {
        let st = stmut(hwnd);
        !st.config.watch_folder.trim().is_empty() && Path::new(&st.config.watch_folder).is_dir()
    }
}

/// Start/restart the in-process Option H chunk sync engine for the Windows UI.
unsafe fn restart_sync_engine(hwnd: HWND) -> std::result::Result<(), String> {
    read_ctrls(hwnd, stmut(hwnd));
    if let Some(cancel) = stmut(hwnd).sync_cancel.take() {
        cancel.store(true, Ordering::Release);
    }
    if let Some(engine) = stmut(hwnd).sync_engine.take() {
        engine.stop();
    }
    if !ensure_or_prompt_watch_folder(hwnd) {
        return Err("Sync not started: choose a valid backup folder on this PC.".into());
    }
    let cfg = stmut(hwnd).config.clone();
    if !is_sync_configured(&cfg) {
        return Err("Sync not started: pair this computer and choose a folder.".into());
    }
    {
        let st = stmut(hwnd);
        st.sync_status_text = "Starting...".into();
        st.sync_status_state = UiSyncState::Checking as usize;
        st.sync_progress_done = 0;
        st.sync_progress_total = 0;
        let _ = st.app.send(crate::app::AppCommand::EngineStarting);
    }

    let engine = crate::sync::SyncEngine::start(cfg.clone())?;
    stmut(hwnd).sync_engine = Some(engine);
    stmut(hwnd).sync_status_text = "Syncing...".into();
    stmut(hwnd).sync_status_state = UiSyncState::Syncing as usize;
    logs::append(&format!(
        "Chunk sync started for {} -> {}",
        cfg.watch_folder, cfg.chunk_endpoint
    ));
    Ok(())
}

unsafe fn read_ctrls(hwnd: HWND, st: &mut WndState) {
    st.config.watch_folder = gettext(hwnd, IDC_WATCH_FOLDER);
    st.config.start_with_windows = checked(hwnd, IDC_START_WINDOWS);
    st.config.auto_update = checked(hwnd, IDC_AUTO_UPDATE);
    let raw_plane = gettext(hwnd, IDC_PAIR_API_BASE);
    if !raw_plane.trim().is_empty() {
        if let Ok(normalized) = crate::config::normalize_pair_api_base(&raw_plane) {
            st.config.pair_api_base = normalized;
            let _ = SetWindowTextW(
                GetDlgItem(hwnd, IDC_PAIR_API_BASE as i32),
                &hstring(&st.config.pair_api_base),
            );
        }
    }
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

/// Match mockup CSS `font-size: Npx` (character height in device pixels).
unsafe fn mkfont_px(name: &str, px: i32, weight: i32) -> HFONT {
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW {
        lfHeight: -px,
        lfWeight: weight,
        ..Default::default()
    };
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

/// Match mockup CSS `font-size: Npx` with underline.
unsafe fn mkfont_px_underline(name: &str, px: i32, weight: i32) -> HFONT {
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW {
        lfHeight: -px,
        lfWeight: weight,
        lfUnderline: 1,
        ..Default::default()
    };
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

fn hstring(s: &str) -> HSTRING {
    HSTRING::from(s)
}

fn approval_timestamp_now() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let st = GetLocalTime();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
        )
    }
}

unsafe fn msgbox_yn(hwnd: HWND, text: &str, title: &str) -> bool {
    MessageBoxW(
        hwnd,
        &hstring(text),
        &hstring(title),
        MB_YESNO | MB_ICONQUESTION,
    )
    .0 == IDYES.0
}

/// Non-blocking notice: writes to `logs/` and Recent Activity (does not freeze the UI).
unsafe fn notify_user(hwnd: HWND, message: &str) {
    logs::append(message);
    let s = Box::new(format!("! {message}"));
    PostMessageW(
        hwnd,
        WM_APP_LOG,
        WPARAM(0),
        LPARAM(Box::into_raw(s) as isize),
    )
    .ok();
}

unsafe fn notify_user_status(hwnd: HWND, status: &str, dot_color: u32, message: &str) {
    set_status_strip_text(hwnd, status);
    set_status_dot_color(hwnd, dot_color);
    notify_user(hwnd, message);
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

fn validate_https_url(url: &str, label: &str) -> std::result::Result<(), String> {
    if url.trim().to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err(format!("{label} must start with https://"))
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}
