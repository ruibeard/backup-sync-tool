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
        IDC_BROWSE_LOCAL => browse_local(hwnd),
        IDC_CONNECT => do_connect(hwnd),
        IDC_PAIR_DEVICE => do_pair_device(hwnd),
        IDC_SAVE => do_save(hwnd),
        IDC_UPDATE_LINK => do_update(hwnd),
        IDC_GITHUB => do_open_repo(hwnd),
        _ => {}
    }
    LRESULT(0)
}

unsafe fn browse_local(hwnd: HWND) {
    let title: Vec<u16> = "Select local folder\0".encode_utf16().collect();
    let current_folder = gettext(hwnd, IDC_WATCH_FOLDER);
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
        return;
    }
    let mut buf = [0u16; 260];
    if SHGetPathFromIDListW(pidl, &mut buf).as_bool() {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32), &hstring(&s));
    }
    ILFree(Some(pidl));
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

unsafe fn do_connect(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    let cfg = st.config.clone();
    if let Err(err) = validate_webdav_url(&cfg.webdav_url) {
        msgbox(hwnd, &err, "Connect");
        return;
    }
    let pass = st.password_plain.clone();
    ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
    // Show amber/yellow dot while connecting.
    set_status_dot_color(hwnd, C_AMBER);
    set_status(hwnd, "\u{25cf}");
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        &hstring("Connecting"),
    );
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
    let raw = hwnd.0;
    std::thread::spawn(move || {
        let ok = webdav::test_connection(&cfg, &pass).is_ok();
        PostMessageW(
            HWND(raw),
            WM_APP_CONNECTED,
            WPARAM(if ok { 1 } else { 0 }),
            LPARAM(0),
        )
        .ok();
    });
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
    let raw = hwnd.0;

    let pair_hwnd = GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32);
    let _ = SetWindowTextW(pair_hwnd, &hstring("Waiting..."));
    EnableWindow(pair_hwnd, FALSE);
    ShowWindow(GetDlgItem(hwnd, IDC_SAVE as i32), SW_HIDE);
    set_status_dot_color(hwnd, C_AMBER);
    set_status(hwnd, "\u{25cf}");
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        &hstring("Waiting for approval"),
    );
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);

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
                        HWND(raw),
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
            PostMessageW(HWND(raw), WM_APP_PAIR_RESULT, WPARAM(ok), LPARAM(payload)).ok();
        }
    });
}

unsafe fn do_save(hwnd: HWND) {
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
            GetDlgItem(hwnd, IDC_URL as i32),
            &hstring(&st.config.webdav_url),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_USERNAME as i32),
            &hstring(&st.config.username),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_PASSWORD as i32),
            &hstring(&st.password_plain),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&st.config.remote_folder),
        );
    }
    if st.config.watch_folder.trim().is_empty() {
        msgbox(hwnd, "Origin folder is required.", "Save");
        return;
    }
    if st.config.webdav_url.trim().is_empty() {
        msgbox(hwnd, "Server URL is required.", "Save");
        return;
    }
    if let Err(err) = validate_webdav_url(&st.config.webdav_url) {
        msgbox(hwnd, &err, "Save");
        return;
    }
    if st.config.remote_folder.trim().is_empty() {
        msgbox(hwnd, "Destination folder is required.", "Save");
        return;
    }
    match secret::encrypt(&st.password_plain) {
        Ok(enc) => st.config.password_enc = enc,
        Err(e) => {
            msgbox(hwnd, &format!("Encrypt error: {e}"), "Error");
            return;
        }
    }
    if let Err(e) = crate::config::save(&st.config) {
        msgbox(hwnd, &format!("Save error: {e}"), "Error");
        return;
    }
    apply_startup(&st.config);
    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    let raw = hwnd.0;
    let log: crate::sync::LogFn = Arc::new(move |m: String| {
        logs::append(&m);
        let s = Box::new(m);
        unsafe {
            PostMessageW(
                HWND(raw),
                WM_APP_LOG,
                WPARAM(0),
                LPARAM(Box::into_raw(s) as isize),
            )
            .ok();
        }
    });
    let activity: crate::sync::ActivityFn = Arc::new(move |info| unsafe {
        PostMessageW(
            HWND(raw),
            WM_APP_SYNC_ACTIVITY,
            WPARAM(info.state as usize),
            LPARAM(Box::into_raw(Box::new((info.completed, info.total))) as isize),
        )
        .ok();
    });
    let auth_failed: crate::sync::AuthFailedFn = Arc::new(move || unsafe {
        PostMessageW(HWND(raw), WM_APP_AUTH_FAILED, WPARAM(0), LPARAM(0)).ok();
    });
    if st.sync_engine.is_some() {
        st.sync_engine = None;
    }
    match crate::sync::SyncEngine::start(cfg.clone(), pass.clone(), log, activity, auth_failed) {
        Ok(e) => {
            let st = stmut(hwnd);
            st.sync_engine = Some(e);
            let msg = Box::new("Settings saved. File watching is active.".to_string());
            PostMessageW(
                HWND(raw),
                WM_APP_LOG,
                WPARAM(0),
                LPARAM(Box::into_raw(msg) as isize),
            )
            .ok();
            msgbox(hwnd, "Settings saved. Sync is now active.", "Save");
        }
        Err(e) => {
            msgbox(hwnd, &format!("Sync error: {e}"), "Error");
        }
    }
    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
        set_status_dot_color(hwnd, C_AMBER);
        set_status(hwnd, "\u{25cf}"); // connecting dot
        ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
        std::thread::spawn(move || {
            let ok = webdav::test_connection(&cfg, &pass).is_ok();
            PostMessageW(
                HWND(raw),
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
        let raw = hwnd.0;
        std::thread::spawn(move || {
            let _ = crate::updater::download_and_replace(&url, |pct| {
                let m = Box::new(format!("Downloading: {pct}%"));
                unsafe {
                    PostMessageW(
                        HWND(raw),
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
    let _ = ShellExecuteW(
        hwnd,
        w!("open"),
        &hstring(REPO_URL),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

unsafe fn do_open_author(hwnd: HWND) {
    let _ = ShellExecuteW(
        hwnd,
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
    let _ = ShellExecuteW(hwnd, w!("open"), &dir_w, None, None, SW_SHOWNORMAL);
}

fn required_client_height(st: &WndState) -> i32 {
    let folders_h = (LBL_H + 4) + INP_H + GAP + (LBL_H + 4) + INP_H + SECT;
    let activity_h =
        HDR_H + PAD + MIN_ACTIVITY_LIST_H + st.post_list_gap + st.sync_row_h + st.post_sync_sect;
    M + 4 + HDR_H + PAD + folders_h + activity_h + st.bottom_bar_h
}

unsafe fn layout_main(hwnd: HWND) {
    let st = state_ptr(hwnd);
    if st.is_null() {
        return;
    }

    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let client_h = cr.bottom - cr.top;

    let mut y = M + 4;

    let status_w = 16i32;
    let pair_x = M + INNER_W - PAIR_BTN_W;
    let server_status_w = SERVER_STATUS_W;
    let server_status_x = pair_x - PAD - server_status_w;
    let status_x = server_status_x - status_w - 4;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SERVER_HDR as i32),
        None,
        M,
        y,
        90,
        HDR_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        None,
        server_status_x,
        y,
        server_status_w,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_STATUS_TEXT as i32),
        None,
        status_x,
        y,
        status_w,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32),
        None,
        pair_x,
        y + (HDR_H - SMALL_BTN_H) / 2,
        PAIR_BTN_W,
        SMALL_BTN_H,
        SWP_NOZORDER,
    )
    .ok();
    y += HDR_H + PAD;

    if !(*st).dividers.is_empty() {
        (&mut (*st).dividers)[0] = y - SECT / 2;
    }

    let browse_x = M + INNER_W - BROWSE_W;
    let inp_w = INNER_W - BROWSE_W - PAD;

    SetWindowPos(
        GetDlgItem(hwnd, IDC_ORIGIN_LABEL as i32),
        None,
        M,
        y,
        INNER_W,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    y += LBL_H + 4;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32),
        None,
        M,
        y,
        inp_w,
        INP_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_BROWSE_LOCAL as i32),
        None,
        browse_x,
        y,
        34,
        INP_H,
        SWP_NOZORDER,
    )
    .ok();
    y += INP_H + GAP;

    SetWindowPos(
        GetDlgItem(hwnd, IDC_DEST_LABEL as i32),
        None,
        M,
        y,
        150,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_DEST_CREATED as i32),
        None,
        M + 118,
        y,
        120,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    y += LBL_H + 4;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
        None,
        M,
        y,
        INNER_W,
        INP_H,
        SWP_NOZORDER,
    )
    .ok();
    y += INP_H + SECT;

    if (*st).dividers.len() > 1 {
        (&mut (*st).dividers)[1] = y - SECT / 2;
    }

    SetWindowPos(
        GetDlgItem(hwnd, IDC_ACTIVITY_HDR as i32),
        None,
        M,
        y,
        INNER_W,
        HDR_H,
        SWP_NOZORDER,
    )
    .ok();
    y += HDR_H + PAD;
    (*st).activity_list_top = y;

    let footer_top = client_h - (*st).bottom_bar_h;
    let activity_fixed_h = (*st).post_list_gap + (*st).sync_row_h + (*st).post_sync_sect;
    let new_lb_h = (footer_top - y - activity_fixed_h).max(MIN_ACTIVITY_LIST_H);
    SetWindowPos(
        GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32),
        None,
        M,
        y,
        INNER_W,
        new_lb_h,
        SWP_NOZORDER,
    )
    .ok();
    y += new_lb_h + (*st).post_list_gap;

    let sync_icon_w = 16i32;
    let sync_gap = 8i32;
    let progress_h = 10i32;
    let sync_row_h = (*st).sync_row_h;
    (*st).sync_icon_rect = RECT {
        left: M,
        top: y + (sync_row_h - sync_icon_w) / 2,
        right: M + sync_icon_w,
        bottom: y + (sync_row_h - sync_icon_w) / 2 + sync_icon_w,
    };

    let status_x = M + sync_icon_w + sync_gap;
    let status_w = 180i32;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_STATUS as i32),
        None,
        status_x,
        y + (sync_row_h - LBL_H) / 2,
        status_w,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();

    let progress_x = status_x + status_w + sync_gap;
    let progress_w = INNER_W - (progress_x - M);
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32),
        None,
        progress_x,
        y + (sync_row_h - progress_h) / 2,
        progress_w,
        progress_h,
        SWP_NOZORDER,
    )
    .ok();
    y += sync_row_h + (*st).post_sync_sect;

    let div_idx = (*st).divider_activity_idx;
    if div_idx < (*st).dividers.len() {
        (&mut (*st).dividers)[div_idx] = y - (*st).post_sync_sect / 2;
    }

    y = footer_top;
    let row_h = BTN_H;
    let button_y = y + (row_h - BTN_H) / 2;
    let check_y = y + (row_h - 18) / 2;
    let save_w = 64i32;
    let save_x = M + INNER_W - save_w;
    let startup_x = M;
    let startup_w = 126i32;
    let two_way_x = startup_x + startup_w + 12;
    let two_way_w = save_x - two_way_x - 12;

    SetWindowPos(
        GetDlgItem(hwnd, IDC_START_WINDOWS as i32),
        None,
        startup_x,
        check_y,
        startup_w,
        18,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_REMOTE as i32),
        None,
        two_way_x,
        check_y,
        two_way_w,
        18,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SAVE as i32),
        None,
        save_x,
        button_y,
        save_w,
        BTN_H,
        SWP_NOZORDER,
    )
    .ok();

    y += row_h;

    let footer_h = LBL_H;
    let footer_y = y + 2;
    let update_btn_w = 26i32;
    let update_btn_h = 20i32;
    let github_btn_w = 20i32;
    let version_w = 72i32;
    let version_x = M;
    let github_btn_x = version_x + version_w + 4;
    let update_btn_x = github_btn_x + github_btn_w + 4;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_REPO as i32),
        None,
        version_x,
        footer_y,
        version_w,
        footer_h,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_GITHUB as i32),
        None,
        github_btn_x,
        footer_y,
        github_btn_w,
        footer_h,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_UPDATE_LINK as i32),
        None,
        update_btn_x,
        footer_y + (footer_h - update_btn_h) / 2,
        update_btn_w,
        update_btn_h,
        SWP_NOZORDER,
    )
    .ok();
    let author_h = 14i32;
    let author_y = footer_y;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_AUTHOR as i32),
        None,
        update_btn_x + update_btn_w + 12,
        author_y,
        200,
        author_h,
        SWP_NOZORDER,
    )
    .ok();

    InvalidateRect(hwnd, None, TRUE);
}

