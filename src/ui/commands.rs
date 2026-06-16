unsafe fn on_command(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let id = (wp.0 & 0xFFFF) as u16;
    let notif = (wp.0 >> 16) as u16;

    if notif == STN_CLICKED as u16 {
        match id {
            IDC_REPO => {
                do_open_repo(hwnd);
                return LRESULT(0);
            }
            IDC_AUTHOR => {
                do_open_author(hwnd);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    if notif == BN_CLICKED as u16 {
        match id {
            IDC_START_WINDOWS | IDC_SYNC_REMOTE => {
                persist_settings_on_toggle(hwnd, id);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    match id {
        x if x == tray::ID_TRAY_OPEN as u16 => {
            ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        x if x == tray::ID_TRAY_LOGS as u16 => {
            do_open_logs(hwnd);
        }
        x if x == tray::ID_TRAY_EXIT as u16 => {
            DestroyWindow(hwnd).ok();
        }
        IDC_BROWSE_LOCAL => {
            browse_local(hwnd, true);
        }
        IDC_OPEN_LOCAL_FOLDER => do_open_local_folder(hwnd),
        IDC_UPDATE_LINK => do_update(hwnd),
        IDC_GITHUB => do_open_repo(hwnd),
        IDC_PAIR_DEVICE => do_pair_device(hwnd),
        IDC_RETRY_FAILED => do_retry_failed_uploads(hwnd),
        _ => {}
    }
    LRESULT(0)
}

unsafe fn browse_local(hwnd: HWND, persist_after_select: bool) -> bool {
    let title: Vec<u16> = "Select local folder\0".encode_utf16().collect();
    let previous_folder = gettext(hwnd, IDC_WATCH_FOLDER);
    let current_folder = previous_folder.clone();
    let initial_folder = if !current_folder.trim().is_empty() && Path::new(&current_folder).is_dir()
    {
        Some(
            current_folder
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>(),
        )
    } else {
        None
    };
    let mut display = [0u16; 260];
    let bi = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        pszDisplayName: PWSTR(display.as_mut_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: Some(browse_local_init_cb),
        lParam: LPARAM(
            initial_folder
                .as_ref()
                .map(|path| path.as_ptr() as isize)
                .unwrap_or(0),
        ),
        ..Default::default()
    };
    let pidl = SHBrowseForFolderW(&bi);
    if pidl.is_null() {
        return false;
    }
    let mut selected = false;
    let mut buf = [0u16; 260];
    if SHGetPathFromIDListW(pidl, &mut buf).as_bool() {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        if !s.trim().is_empty() {
            selected = true;
        }
        if s != previous_folder {
            let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32), &hstring(&s));
            invalidate_bridge(hwnd);
            if persist_after_select && !s.trim().is_empty() {
                persist_settings(hwnd, true);
            }
        }
    }
    ILFree(Some(pidl));
    selected
}

unsafe fn do_open_local_folder(hwnd: HWND) {
    let folder = gettext(hwnd, IDC_WATCH_FOLDER);
    let folder = folder.trim();
    if folder.is_empty() {
        notify_user(hwnd, "Origin folder is empty.");
        return;
    }
    if !Path::new(folder).is_dir() {
        notify_user(hwnd, "Origin folder does not exist.");
        return;
    }
    let _ = windows::Win32::UI::Shell::ShellExecuteW(
        Some(hwnd),
        w!("open"),
        &hstring(folder),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

unsafe extern "system" fn browse_local_init_cb(
    hwnd: HWND,
    msg: u32,
    _lparam: LPARAM,
    data: LPARAM,
) -> i32 {
    if msg == BFFM_INITIALIZED && data.0 != 0 {
        SendMessageW(hwnd, BFFM_SETSELECTIONW, WPARAM(1), data);
    }
    0
}

unsafe fn do_pair_device(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    let api_base = st.config.pair_api_base.clone();
    let mut detected_folder = if st.remote_folder_from_xd {
        non_empty(st.config.remote_folder.clone())
    } else {
        None
    };
    let cancel = Arc::new(AtomicBool::new(false));
    st.pair_id = st.pair_id.wrapping_add(1);
    let pair_id = st.pair_id;
    st.pair_cancel = Some(cancel.clone());
    let raw = hwnd.0 as isize;

    ShowWindow(GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32), SW_HIDE);
    show_pair_qr_window(hwnd);
    set_status_dot_color(hwnd, C_AMBER);
    set_status_strip_text(hwnd, "Pairing \u{00B7} waiting for approval");

    std::thread::spawn(move || {
        if detected_folder.is_none() {
            if let Some(detected) = crate::xd::detect_customer_hint() {
                if let Some(folder) = non_empty(detected.folder) {
                    detected_folder.get_or_insert(folder);
                }
            }
        }
        let machine = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".to_string());
        let windows_user = std::env::var("USERNAME").unwrap_or_default();
        let version = env!("CARGO_PKG_VERSION");
        let result = match crate::pairing::start_pairing(
            &api_base,
            &machine,
            &windows_user,
            version,
            detected_folder,
        ) {
            Some(start) => {
                unsafe {
                    let started = Box::new(PairStarted {
                        pair_id,
                        code: start.code.clone(),
                        approve_url: start.approve_url.clone(),
                    });
                    PostMessageW(
                        HWND(raw as *mut _),
                        WM_APP_PAIR_STARTED,
                        WPARAM(0),
                        LPARAM(Box::into_raw(started) as isize),
                    )
                        .ok();
                }

                let started = std::time::Instant::now();
                let sleep_ms = start.poll_interval_ms.clamp(1000, 10_000);
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break Err(String::new());
                    }
                    if started.elapsed() > std::time::Duration::from_secs(300) {
                        break Err(format!(
                            "Pairing timed out.\nCode: {}\nApprove URL: {}",
                            start.code, start.approve_url
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
                    if cancel.load(Ordering::Relaxed) {
                        break Err(String::new());
                    }
                    if let Some(status) = crate::pairing::poll_pairing(&api_base, &start.poll_token)
                    {
                        match status.status.as_str() {
                            "approved" => {
                                let device_token =
                                    match required_pair_field(status.device_token, "device token") {
                                        Ok(value) => value,
                                        Err(err) => break Err(err),
                                    };
                                let webdav_url =
                                    match required_pair_field(status.webdav_url, "server URL") {
                                        Ok(value) => value,
                                        Err(err) => break Err(err),
                                    };
                                if let Err(err) = validate_webdav_url(&webdav_url) {
                                    break Err(format!("Pairing approved with invalid server URL: {err}"));
                                }
                                let username =
                                    match required_pair_field(status.username, "username") {
                                        Ok(value) => value,
                                        Err(err) => break Err(err),
                                    };
                                let password =
                                    match required_pair_field(status.password, "password") {
                                        Ok(value) => value,
                                        Err(err) => break Err(err),
                                    };
                                let remote_folder =
                                    match approved_remote_folder(status.remote_folder.as_deref()) {
                                        Ok(folder) => folder,
                                        Err(err) => break Err(err),
                                    };
                                break Ok(PairResult {
                                    pair_id,
                                    device_token,
                                    webdav_url,
                                    username,
                                    password,
                                    remote_folder,
                                    credential_profile_id: status.credential_profile_id,
                                    credential_version: status.credential_version,
                                });
                            }
                            "rejected" => break Err("Pairing was rejected.".to_string()),
                            "expired" => break Err("Pairing request expired. Start pairing again.".to_string()),
                            "consumed" => break Err("Pairing payload was already consumed. Start pairing again.".to_string()),
                            "failed" => break Err("Pairing was approved but the server payload is missing. Start pairing again.".to_string()),
                            _ => {}
                        }
                    }
                }
            }
            None => Err(format!("Could not start pairing at {api_base}.")),
        };

        let (ok, payload): (usize, isize) = match result {
            Ok(pair) => (1, Box::into_raw(Box::new(pair)) as isize),
            Err(message) => (
                0,
                Box::into_raw(Box::new(PairError { pair_id, message })) as isize,
            ),
        };
        unsafe {
            PostMessageW(HWND(raw as *mut _), WM_APP_PAIR_RESULT, WPARAM(ok), LPARAM(payload)).ok();
        }
    });
}

unsafe fn persist_settings_on_toggle(hwnd: HWND, id: u16) {
    let st = stmut(hwnd);
    let prev_start = st.config.start_with_windows;
    let prev_sync = st.config.sync_remote_changes;
    read_ctrls(hwnd, st);
    if id == IDC_START_WINDOWS && st.config.start_with_windows == prev_start {
        return;
    }
    if id == IDC_SYNC_REMOTE && st.config.sync_remote_changes == prev_sync {
        return;
    }
    persist_settings(hwnd, false);
}

unsafe fn persist_settings(hwnd: HWND, notify_ok: bool) {
    let st = stmut(hwnd);
    let was_paired = is_paired(&st.config);
    let locked_webdav_url = st.config.webdav_url.clone();
    let locked_username = st.config.username.clone();
    let locked_password = st.password_plain.clone();
    let locked_remote_folder = st.config.remote_folder.clone();
    read_ctrls(hwnd, st);
    if was_paired {
        st.config.webdav_url = locked_webdav_url;
        st.config.username = locked_username;
        st.password_plain = locked_password;
        st.config.remote_folder = locked_remote_folder;
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&st.config.remote_folder),
        );
    }
    if st.config.watch_folder.trim().is_empty() {
        notify_user(hwnd, "Origin folder is required.");
        return;
    }
    if st.config.webdav_url.trim().is_empty() {
        notify_user(hwnd, "Server URL is required.");
        return;
    }
    if let Err(err) = validate_webdav_url(&st.config.webdav_url) {
        notify_user_status(hwnd, "Save failed", C_RED, &err);
        return;
    }
    if st.config.remote_folder.trim().is_empty() {
        notify_user(hwnd, "Destination folder is required.");
        return;
    }
    match secret::encrypt(&st.password_plain) {
        Ok(enc) => st.config.password_enc = enc,
        Err(e) => {
            notify_user_status(hwnd, "Save failed", C_RED, &format!("Encrypt error: {e}"));
            return;
        }
    }
    if let Err(e) = crate::config::save(&st.config) {
        notify_user_status(hwnd, "Save failed", C_RED, &format!("Save error: {e}"));
        return;
    }
    apply_startup(&st.config);
    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    let raw = hwnd.0 as isize;
    match restart_sync_engine(hwnd) {
        Ok(()) => {
            let st = stmut(hwnd);
            st.sync_status_state = crate::sync::ActivityState::Checking as usize;
            set_status_strip_connection(hwnd);
            if notify_ok {
                notify_user(hwnd, "Settings saved. Sync started.");
            }
        }
        Err(e) => notify_user_status(hwnd, "Sync error", C_RED, &e),
    }
    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        set_status_dot_color(hwnd, C_AMBER);
        std::thread::spawn(move || {
            let ok = webdav::test_connection(&cfg, &pass).is_ok();
            PostMessageW(
                HWND(raw as *mut _),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
                .ok();
        });
    }
}

unsafe fn do_update(hwnd: HWND) {
    let url = match stmut(hwnd).update_url.clone() {
        Some(u) => u,
        None => return,
    };
    if msgbox_yn(
        hwnd,
        "A new version is available.\nDownload and install now? The app will restart.",
        "Update Available",
    ) {
        let msg = Box::new("Update started.".to_string());
        PostMessageW(
            hwnd,
            WM_APP_LOG,
            WPARAM(0),
            LPARAM(Box::into_raw(msg) as isize),
        )
            .ok();
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);
        let raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = crate::updater::download_and_replace(&url, |pct| {
                let m = Box::new(format!("Downloading: {pct}%"));
                unsafe {
                    PostMessageW(
                        HWND(raw as *mut _),
                        WM_APP_LOG,
                        WPARAM(0),
                        LPARAM(Box::into_raw(m) as isize),
                    )
                        .ok();
                }
            });
        });
    }
}

unsafe fn do_open_repo(hwnd: HWND) {
    let _ = windows::Win32::UI::Shell::ShellExecuteW(
        Some(hwnd),
        w!("open"),
        &hstring(REPO_URL),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

unsafe fn do_open_author(hwnd: HWND) {
    let _ = windows::Win32::UI::Shell::ShellExecuteW(
        Some(hwnd),
        w!("open"),
        &hstring(AUTHOR_URL),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

unsafe fn do_open_logs(hwnd: HWND) {
    let dir = logs::ensure_logs_dir();
    let dir_w = hstring(&dir.to_string_lossy());
    let _ = windows::Win32::UI::Shell::ShellExecuteW(
        Some(hwnd),
        w!("open"),
        &dir_w,
        None,
        None,
        SW_SHOWNORMAL,
    );
}

fn client_inner_w(hwnd: HWND) -> i32 {
    let mut cr = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut cr).ok();
    }
    (cr.right - cr.left - M * 2).max(200)
}

fn required_client_height(st: &WndState) -> i32 {
    let bridge_h = bridge_section_total_h(st);
    let activity_h = HDR_H
        + PAD
        + MIN_ACTIVITY_LIST_H
        + st.post_list_gap
        + st.sync_row_h
        + st.post_sync_sect;
    CONTENT_TOP_PAD + bridge_h + activity_h + st.bottom_bar_h
}

/// Grow the window when content (e.g. idle progress block) needs more height.
/// Returns true when a resize was applied; WM_SIZE will have laid out recursively.
unsafe fn ensure_client_height(hwnd: HWND) -> bool {
    let st = state_ptr(hwnd);
    if st.is_null() {
        return false;
    }

    let needed = required_client_height(&*st);
    (*st).min_client_h = needed;

    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let current_h = cr.bottom - cr.top;
    if current_h >= needed {
        return false;
    }

    let mut wr = RECT::default();
    GetWindowRect(hwnd, &mut wr).ok();
    GetClientRect(hwnd, &mut cr).ok();
    let dh = (wr.bottom - wr.top) - (cr.bottom - cr.top);
    let dw = (wr.right - wr.left) - (cr.right - cr.left);
    SetWindowPos(
        hwnd,
        None,
        0,
        0,
        WIN_W + dw,
        needed + dh,
        SWP_NOMOVE | SWP_NOZORDER,
    )
    .ok();
    true
}

unsafe fn layout_main(hwnd: HWND) {
    let st = state_ptr(hwnd);
    if st.is_null() {
        return;
    }

    if ensure_client_height(hwnd) {
        return;
    }

    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let client_h = cr.bottom - cr.top;

    let mut y = CONTENT_TOP_PAD;

    (*st).inner_w = client_inner_w(hwnd);
    (*st).status_strip_rect = RECT::default();

    y = layout_bridge_section(
        hwnd,
        HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _),
        &(*st).config.clone(),
        y,
        (*st).hfont_bridge,
    );

    let sub_w = 180;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_ACTIVITY_HDR as i32),
        None,
        M,
        y,
        (*st).inner_w - sub_w - PAD,
        HDR_H,
        SWP_NOZORDER,
    )
        .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_ACTIVITY_SUBHDR as i32),
        None,
        M + (*st).inner_w - sub_w,
        y,
        sub_w,
        HDR_H,
        SWP_NOZORDER,
    )
        .ok();
    y += HDR_H + PAD;
    (*st).activity_list_top = y;

    let footer_top = client_h - (*st).bottom_bar_h;
    let activity_fixed_h = (*st).post_list_gap + (*st).sync_row_h + (*st).post_sync_sect;
    let available = footer_top - y - activity_fixed_h;
    let new_lb_h = if available >= MIN_ACTIVITY_LIST_H {
        available
    } else {
        available.max(0)
    };
    (*st).activity_list_rect = RECT {
        left: M,
        top: y,
        right: M + (*st).inner_w,
        bottom: y + new_lb_h,
    };
    SetWindowPos(
        GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32),
        None,
        M + 1,
        y + 1,
        (*st).inner_w - 2,
        new_lb_h - 2,
        SWP_NOZORDER,
    )
        .ok();
    y += new_lb_h + (*st).post_list_gap;
    (*st).sync_footer_rect = RECT {
        left: M,
        top: y,
        right: M + (*st).inner_w,
        bottom: y + (*st).sync_row_h,
    };
    let footer_pad_x = 10;
    let footer_pad_y = 8;
    let retry_btn_x = M + (*st).inner_w - footer_pad_x - ACTION_BTN_W;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_RETRY_FAILED as i32),
        None,
        retry_btn_x,
        y + footer_pad_y,
        ACTION_BTN_W,
        ACTION_BTN_H,
        SWP_NOZORDER,
    )
        .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_STATUS as i32),
        None,
        M + footer_pad_x,
        y + footer_pad_y,
        retry_btn_x - M - footer_pad_x - PAD,
        LBL_H,
        SWP_NOZORDER,
    )
        .ok();
    y += (*st).sync_row_h;
    y += (*st).post_sync_sect;

    y = footer_top;
    (*st).footer_panel_rect = RECT {
        left: 0,
        top: footer_top.saturating_sub(1),
        right: M + (*st).inner_w + M,
        bottom: client_h,
    };
    let row_h = BTN_H;
    let check_h = 22;
    let check_y = y + (row_h - check_h) / 2;
    let startup_x = M;
    let startup_w = 180i32;
    let two_way_x = startup_x + startup_w + 12;
    let two_way_w = M + (*st).inner_w - two_way_x;

    SetWindowPos(
        GetDlgItem(hwnd, IDC_START_WINDOWS as i32),
        None,
        startup_x,
        check_y,
        startup_w,
        check_h,
        SWP_NOZORDER,
    )
        .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_REMOTE as i32),
        None,
        two_way_x,
        check_y,
        two_way_w,
        check_h,
        SWP_NOZORDER,
    )
        .ok();
    y += row_h;

    let footer_y = y + 2;
    let meta = footer_meta_layout(footer_y, (*st).hfont_link, "Rui Almeida");
    position_footer_meta(hwnd, &meta, "Rui Almeida");

    InvalidateRect(hwnd, None, TRUE);
}
