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
            IDC_START_WINDOWS | IDC_AUTO_UPDATE => {
                persist_settings_on_toggle(hwnd, id);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    if notif == EN_KILLFOCUS as u16 && id == IDC_PAIR_API_BASE {
        persist_pair_api_base_on_blur(hwnd);
        return LRESULT(0);
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
        _ => {}
    }
    LRESULT(0)
}

unsafe fn apply_pairing_folder_hint(hwnd: HWND, watch_folder: &str) {
    if is_paired(&stmut(hwnd).config) {
        return;
    }

    let use_xd = crate::xd::is_xd_default_watch_folder(watch_folder);
    let (folder, customer, from_xd) = if use_xd {
        if let Some(detected) = crate::xd::detect_customer_hint() {
            (detected.folder, non_empty(detected.customer), true)
        } else if let Some(hint) = crate::xd::build_host_folder_hint(watch_folder) {
            (hint, None, false)
        } else {
            return;
        }
    } else if let Some(hint) = crate::xd::build_host_folder_hint(watch_folder) {
        (hint, None, false)
    } else {
        return;
    };

    let st = stmut(hwnd);
    st.detected_customer = customer;
    st.remote_folder_from_xd = from_xd;
    let display = st
        .detected_customer
        .clone()
        .unwrap_or(folder);
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
        &hstring(&display),
    );
    invalidate_bridge(hwnd);
    update_server_tooltip(hwnd);
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
            read_ctrls(hwnd, stmut(hwnd));
            stmut(hwnd)
                .app
                .send(crate::app::AppCommand::SetWatchFolder(
                    std::path::PathBuf::from(&s),
                ))
                .ok();
            update_pair_button_enabled(hwnd);
            apply_pairing_folder_hint(hwnd, &s);
            layout_main(hwnd);
            if persist_after_select && !s.trim().is_empty() {
                if is_paired(&stmut(hwnd).config) {
                    persist_settings(hwnd, true);
                } else if let Err(err) = crate::config::save(&stmut(hwnd).config) {
                    notify_user_status(hwnd, "Save failed", C_RED, &format!("Save error: {err}"));
                } else {
                    notify_user(hwnd, "Backup folder selected.");
                }
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
    read_ctrls(hwnd, stmut(hwnd));
    {
        let st = stmut(hwnd);
        match crate::config::normalize_pair_api_base(&st.config.pair_api_base) {
            Ok(normalized) => {
                st.config.pair_api_base = normalized;
                let _ = SetWindowTextW(
                    GetDlgItem(hwnd, IDC_PAIR_API_BASE as i32),
                    &hstring(&st.config.pair_api_base),
                );
                if let Err(e) = crate::config::save(&st.config) {
                    notify_user_status(
                        hwnd,
                        "Could not save control plane URL",
                        C_RED,
                        &format!("{e}"),
                    );
                    return;
                }
            }
            Err(err) => {
                notify_user_status(hwnd, "Control plane URL", C_AMBER, &err);
                return;
            }
        }
    }
    if !watch_folder_is_valid(&stmut(hwnd).config.watch_folder) {
        update_pair_button_enabled(hwnd);
        notify_user_status(
            hwnd,
            "Backup folder required",
            C_AMBER,
            "Choose a valid backup folder on this PC before connecting the server.",
        );
        let _ = SetForegroundWindow(hwnd);
        return;
    }
    let st = stmut(hwnd);
    st.app.send(crate::app::AppCommand::Connect).ok();
    let api_base = st.config.pair_api_base.clone();
    let watch_folder = st.config.watch_folder.clone();
    // XD name only while watch is still C:\XDSoftware\backups.
    // Any other Chosen folder → hostname-folder, XD ignored.
    let xd = if crate::xd::is_xd_default_watch_folder(&watch_folder) {
        crate::xd::detect_customer_hint()
    } else {
        None
    };
    let detected_folder = xd
        .as_ref()
        .and_then(|d| {
            let folder = d.folder.trim();
            (!folder.is_empty()).then(|| d.folder.clone())
        })
        .or_else(|| crate::xd::build_host_folder_hint(&watch_folder));
    if let Some(folder) = detected_folder.as_ref() {
        st.remote_folder_from_xd = xd.is_some();
        st.detected_customer = xd.as_ref().and_then(|d| non_empty(d.customer.clone()));
        let display = st.detected_customer.clone().unwrap_or_else(|| folder.clone());
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&display),
        );
        invalidate_bridge(hwnd);
        update_server_tooltip(hwnd);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    st.pair_id = st.pair_id.wrapping_add(1);
    let pair_id = st.pair_id;
    st.pair_cancel = Some(cancel.clone());
    let raw = hwnd.0 as isize;

    ShowWindow(GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32), SW_HIDE);
    show_pair_qr_window(hwnd);
    set_status_dot_color(hwnd, C_AMBER);
    set_status_strip_text(hwnd, &format!("Pairing \u{00B7} {api_base}"));
    logs::append(&format!("Pairing with control plane: {api_base}"));

    std::thread::spawn(move || {
        let mut approval_received = false;
        let machine = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".to_string());
        let windows_user = std::env::var("USERNAME").unwrap_or_default();
        let version = env!("CARGO_PKG_VERSION");
        let result = match crate::pairing::start_pairing_cancellable(
            &api_base,
            &machine,
            &windows_user,
            version,
            xd.as_ref().and_then(|_| crate::xd::install_path()),
            Some(watch_folder.clone()),
            xd.as_ref().map(|detected| detected.number.clone()),
            xd.as_ref().map(|detected| detected.customer.clone()),
            detected_folder,
            &cancel,
        ) {
            Ok(start) => {
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
                let mut poll_error_logged = false;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break Err(String::new());
                    }
                    if started.elapsed() > std::time::Duration::from_secs(600) {
                        break Err(format!(
                            "Pairing timed out.\nCode: {}\nApprove URL: {}",
                            start.code, start.approve_url
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
                    if cancel.load(Ordering::Relaxed) {
                        break Err(String::new());
                    }
                    match crate::pairing::poll_pairing_cancellable(
                        &api_base,
                        &start.poll_token,
                        &cancel,
                    ) {
                        Ok(status) => {
                            poll_error_logged = false;
                            match status.status.as_str() {
                            "approved" => {
                                approval_received = true;
                                if !crate::pairing::is_chunk_store_approval(&status) {
                                    break Err(
                                        "Pairing approved without a chunk_store assignment. Pair again."
                                            .to_string(),
                                    );
                                }
                                let device_token =
                                    match required_pair_field(status.device_token, "device token") {
                                        Ok(value) => value,
                                        Err(err) => break Err(err),
                                    };
                                let device_uuid = match required_pair_field(
                                    status.device_uuid,
                                    "device UUID",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let destination_uuid = match required_pair_field(
                                    status.destination_uuid,
                                    "destination UUID",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let destination_label = match required_pair_field(
                                    status.destination_label,
                                    "destination label",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let chunk_endpoint = match required_pair_field(
                                    status.chunk_endpoint,
                                    "chunk endpoint",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let chunk_bucket = match required_pair_field(
                                    status.chunk_bucket,
                                    "chunk bucket",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let chunk_access_key = match required_pair_field(
                                    status.chunk_access_key,
                                    "chunk access key",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let chunk_secret_key = match required_pair_field(
                                    status.chunk_secret_key,
                                    "chunk secret key",
                                ) {
                                    Ok(value) => value,
                                    Err(err) => break Err(err),
                                };
                                let pair = PairResult {
                                    pair_id,
                                    device_uuid,
                                    device_token,
                                    destination_uuid,
                                    destination_label,
                                    chunk_endpoint,
                                    chunk_region: status
                                        .chunk_region
                                        .unwrap_or_else(|| "garage".into()),
                                    chunk_bucket,
                                    chunk_prefix: status.chunk_prefix.unwrap_or_default(),
                                    chunk_access_key,
                                    chunk_secret_key,
                                    chunk_path_style: status.chunk_path_style.unwrap_or(true),
                                };
                                break Ok(pair);
                            }
                            "rejected" => break Err("Pairing was rejected.".to_string()),
                            "expired" => break Err("Pairing request expired. Start pairing again.".to_string()),
                            "consumed" => {
                                approval_received = true;
                                break Err("Pairing payload was already consumed. Start pairing again.".to_string());
                            }
                            "failed" => break Err("Pairing was approved but the server payload is missing. Start pairing again.".to_string()),
                            _ => {}
                        }
                        }
                        Err(err) if err.kind == crate::pairing::PairingErrorKind::Cancelled => {
                            break Err(String::new());
                        }
                        Err(err) if err.is_transient() => {
                            if !poll_error_logged {
                                logs::append(&format!(
                                    "Pair poll temporarily failed; retrying: {err}"
                                ));
                                poll_error_logged = true;
                            }
                        }
                        Err(err) => break Err(err.to_string()),
                    }
                }
            }
            Err(err) if err.kind == crate::pairing::PairingErrorKind::Cancelled => {
                Err(String::new())
            }
            Err(err) => Err(err.to_string()),
        };

        let (ok, payload): (usize, isize) = match result {
            Ok(pair) => (1, Box::into_raw(Box::new(pair)) as isize),
            Err(message) => (
                0,
                Box::into_raw(Box::new(PairError {
                    pair_id,
                    message,
                    approval_received,
                })) as isize,
            ),
        };
        unsafe {
            PostMessageW(
                HWND(raw as *mut _),
                WM_APP_PAIR_RESULT,
                WPARAM(ok),
                LPARAM(payload),
            )
            .ok();
        }
    });
}

unsafe fn persist_pair_api_base_on_blur(hwnd: HWND) {
    let raw = gettext(hwnd, IDC_PAIR_API_BASE);
    let st = stmut(hwnd);
    if raw.trim().is_empty() {
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_PAIR_API_BASE as i32),
            &hstring(&st.config.pair_api_base),
        );
        return;
    }
    match crate::config::normalize_pair_api_base(&raw) {
        Ok(normalized) => {
            let changed = normalized != st.config.pair_api_base;
            st.config.pair_api_base = normalized;
            let _ = SetWindowTextW(
                GetDlgItem(hwnd, IDC_PAIR_API_BASE as i32),
                &hstring(&st.config.pair_api_base),
            );
            if !changed {
                return;
            }
            if let Err(e) = crate::config::save(&st.config) {
                notify_user_status(
                    hwnd,
                    "Control plane",
                    C_RED,
                    &format!("Could not save Control plane URL: {e}"),
                );
            } else {
                logs::append(&format!(
                    "Control plane URL set: {}",
                    st.config.pair_api_base
                ));
            }
        }
        Err(err) => {
            let _ = SetWindowTextW(
                GetDlgItem(hwnd, IDC_PAIR_API_BASE as i32),
                &hstring(&st.config.pair_api_base),
            );
            notify_user_status(hwnd, "Control plane", C_RED, &err);
        }
    }
}

unsafe fn persist_settings_on_toggle(hwnd: HWND, id: u16) {
    let st = stmut(hwnd);
    let prev_start = st.config.start_with_windows;
    let prev_auto_update = st.config.auto_update;
    read_ctrls(hwnd, st);
    if id == IDC_START_WINDOWS && st.config.start_with_windows == prev_start {
        return;
    }
    if id == IDC_AUTO_UPDATE && st.config.auto_update == prev_auto_update {
        return;
    }
    if id == IDC_AUTO_UPDATE {
        if let Err(e) = crate::config::save(&st.config) {
            notify_user_status(hwnd, "Save failed", C_RED, &format!("Save error: {e}"));
        }
        return;
    }
    persist_settings(hwnd, false);
}

unsafe fn persist_settings(hwnd: HWND, notify_ok: bool) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    if st.config.watch_folder.trim().is_empty() {
        notify_user(hwnd, "Origin folder is required.");
        return;
    }
    if let Err(e) = crate::config::save(&st.config) {
        notify_user_status(hwnd, "Save failed", C_RED, &format!("Save error: {e}"));
        return;
    }
    apply_startup(&st.config);
    let result = if is_sync_configured(&st.config) {
        restart_sync_engine(hwnd)
    } else {
        Ok(())
    };
    match result {
        Ok(()) => {
            let st = stmut(hwnd);
            st.sync_status_state = UiSyncState::Checking as usize;
            set_status_strip_connection(hwnd);
            if notify_ok {
                notify_user(hwnd, "Settings saved. Sync started.");
            }
        }
        Err(e) => notify_user_status(hwnd, "Sync error", C_RED, &e),
    }
}

unsafe fn do_update(hwnd: HWND) {
    if stmut(hwnd).repair_required {
        start_bundle_repair(hwnd);
        return;
    }
    let url = match stmut(hwnd).update_url.clone() {
        Some(u) => u,
        None => return,
    };
    if msgbox_yn(
        hwnd,
        "A new version is available.\nDownload and install now? The app will restart.",
        "Update Available",
    ) {
        start_update_install(hwnd, url);
    }
}

unsafe fn start_bundle_repair(hwnd: HWND) {
    let button = GetDlgItem(hwnd, IDC_UPDATE_LINK as i32);
    let _ = SetWindowTextW(button, &hstring("Repairing..."));
    EnableWindow(button, false);
    ShowWindow(button, SW_SHOW);
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        if let Err(error) = crate::updater::repair_current_bundle(|pct| {
            let message = Box::new(format!("Repair download: {pct}%"));
            unsafe {
                PostMessageW(
                    HWND(raw as *mut _),
                    WM_APP_LOG,
                    WPARAM(0),
                    LPARAM(Box::into_raw(message) as isize),
                )
                .ok();
            }
        }) {
            let error = Box::new(error);
            unsafe {
                PostMessageW(
                    HWND(raw as *mut _),
                    WM_APP_REPAIR_FAILED,
                    WPARAM(0),
                    LPARAM(Box::into_raw(error) as isize),
                )
                .ok();
            }
        }
    });
}

unsafe fn start_update_install(hwnd: HWND, url: String) {
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
    let activity_h =
        HDR_H + PAD + MIN_ACTIVITY_LIST_H + st.post_list_gap + st.sync_row_h + st.post_sync_sect;
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
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SYNC_STATUS as i32),
        None,
        M + footer_pad_x,
        y + footer_pad_y,
        (*st).inner_w - footer_pad_x * 2,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
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
    let startup_w = 154i32;
    let auto_update_x = startup_x + startup_w + 12;
    let auto_update_w = 114i32;
    let policy_x = auto_update_x + auto_update_w + 12;
    let policy_w = (M + (*st).inner_w - policy_x).max(1);

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
        GetDlgItem(hwnd, IDC_AUTO_UPDATE as i32),
        None,
        auto_update_x,
        check_y,
        auto_update_w,
        check_h,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_SERVER_DELETION_POLICY as i32),
        None,
        policy_x,
        check_y,
        policy_w,
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
