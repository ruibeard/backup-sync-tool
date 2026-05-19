// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    let Some(entry) = activity_entry(&msg) else {
        return LRESULT(0);
    };
    let hlb = GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32);
    if let Some(previous) = activity_replaces(&msg) {
        let previous = hstring(&previous);
        let idx = SendMessageW(
            hlb,
            LB_FINDSTRINGEXACT,
            WPARAM(usize::MAX),
            LPARAM(previous.as_ptr() as isize),
        );
        if idx.0 >= 0 {
            SendMessageW(hlb, LB_DELETESTRING, WPARAM(idx.0 as usize), LPARAM(0));
        }
    }
    let ws = hstring(&entry);
    SendMessageW(
        hlb,
        LB_INSERTSTRING,
        WPARAM(0),
        LPARAM(ws.as_ptr() as isize),
    );
    if SendMessageW(hlb, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0 > 200 {
        SendMessageW(hlb, LB_DELETESTRING, WPARAM(200), LPARAM(0));
    }
    LRESULT(0)
}

unsafe fn on_app_sync_activity(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let progress = if lp.0 != 0 {
        *Box::from_raw(lp.0 as *mut (usize, usize))
    } else {
        (0, 0)
    };
    let (icon_name, mut status_text) = match wp.0 {
        x if x == crate::sync::ActivityState::Checking as usize => {
            (w!("APP_ICON_SYNCING"), "Checking...")
        }
        x if x == crate::sync::ActivityState::Syncing as usize => {
            (w!("APP_ICON_SYNCING"), "Syncing...")
        }
        _ => (w!("APP_ICON_COMPLETE"), "All synced"),
    };

    let st = stmut(hwnd);
    let was_busy = st.sync_status_state == crate::sync::ActivityState::Checking as usize
        || st.sync_status_state == crate::sync::ActivityState::Syncing as usize;
    let was_syncing = st.sync_status_state == crate::sync::ActivityState::Syncing as usize;
    let is_syncing = wp.0 == crate::sync::ActivityState::Syncing as usize;
    let is_busy = wp.0 == crate::sync::ActivityState::Checking as usize || is_syncing;
    st.sync_status_state = wp.0;
    st.sync_progress_done = progress.0;
    st.sync_progress_total = progress.1;
    if is_busy {
        if is_syncing && !was_syncing {
            st.sync_started_at = Some(std::time::Instant::now());
        }
        if is_syncing && progress.1 > 0 {
            let done = progress.0.min(progress.1);
            let pct = (done * 100) / progress.1;
            let eta = if done > 0 {
                st.sync_started_at.and_then(|started| {
                    let elapsed = started.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        let per_item = elapsed / done as f64;
                        let remaining = ((progress.1 - done) as f64 * per_item).ceil() as u64;
                        Some(format_eta(remaining))
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            st.sync_status_text = if let Some(eta) = eta {
                format!("{done}/{} \u{00B7} ETA {} \u{00B7} {pct}%", progress.1, eta)
            } else {
                format!("{done}/{} \u{00B7} {pct}%", progress.1)
            };
            status_text = &st.sync_status_text;
        }
        if !was_busy {
            st.sync_anim_frame = 0;
            let _ = SetTimer(hwnd, IDT_SYNC_ANIM, SYNC_ANIM_MS, None);
        }
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);
        let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
        if !hicon.0.is_null() {
            tray::set_tray_icon_and_tip(
                hwnd,
                hicon,
                &format!("Backup Sync Tool - {}", status_text),
            );
            st.sync_icon = hicon;
            InvalidateRect(hwnd, Some(&st.sync_icon_rect), TRUE);
        }
    } else {
        st.sync_started_at = None;
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);
        let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
        if !hicon.0.is_null() {
            tray::set_tray_icon_and_tip(hwnd, hicon, "Backup Sync Tool");
            st.sync_icon = hicon;
            InvalidateRect(hwnd, Some(&st.sync_icon_rect), TRUE);
        }
    }
    if !is_syncing {
        st.sync_status_text = status_text.to_string();
    }
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SYNC_STATUS as i32),
        &hstring(&st.sync_status_text),
    );
    let progress_hwnd = GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32);
    if is_syncing && progress.1 > 0 {
        let pct = ((progress.0.min(progress.1) * 100) / progress.1) as isize;
        SendMessageW(progress_hwnd, PBM_SETPOS, WPARAM(pct as usize), LPARAM(0));
        ShowWindow(progress_hwnd, SW_SHOW);
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);
        let tip_icon = LoadIconW(hi, w!("APP_ICON_SYNCING")).unwrap_or_default();
        if !tip_icon.0.is_null() {
            tray::set_tray_icon_and_tip(
                hwnd,
                tip_icon,
                &format!("Backup Sync Tool - {}", st.sync_status_text),
            );
        }
    } else {
        SendMessageW(progress_hwnd, PBM_SETPOS, WPARAM(0), LPARAM(0));
        ShowWindow(progress_hwnd, SW_HIDE);
    }
    InvalidateRect(GetDlgItem(hwnd, IDC_SYNC_STATUS as i32), None, TRUE);
    LRESULT(0)
}

unsafe fn on_timer(hwnd: HWND, wp: WPARAM) -> LRESULT {
    if wp.0 != IDT_SYNC_ANIM {
        return DefWindowProcW(hwnd, WM_TIMER, wp, LPARAM(0));
    }

    let st = stmut(hwnd);
    if st.sync_status_state != crate::sync::ActivityState::Checking as usize
        && st.sync_status_state != crate::sync::ActivityState::Syncing as usize
    {
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
        return LRESULT(0);
    }

    let names = [
        w!("APP_ICON_SYNC_1"),
        w!("APP_ICON_SYNC_2"),
        w!("APP_ICON_SYNC_3"),
        w!("APP_ICON_SYNC_4"),
        w!("APP_ICON_SYNC_5"),
        w!("APP_ICON_SYNC_6"),
    ];
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);
    let icon_name = names[st.sync_anim_frame % names.len()];
    st.sync_anim_frame = (st.sync_anim_frame + 1) % names.len();
    let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
    if !hicon.0.is_null() {
        let tip = if !st.sync_status_text.is_empty() {
            format!("Backup Sync Tool - {}", st.sync_status_text)
        } else {
            "Backup Sync Tool - Syncing".to_string()
        };
        tray::set_tray_icon_and_tip(hwnd, hicon, &tip);
        st.sync_icon = hicon;
        InvalidateRect(hwnd, Some(&st.sync_icon_rect), TRUE);
    }
    LRESULT(0)
}

unsafe fn on_app_connected(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let connected = wp.0 == 1;
    let st = stmut(hwnd);
    st.connected = connected;
    let paired = is_paired(&st.config);
    let status_hwnd = GetDlgItem(hwnd, IDC_STATUS_TEXT as i32);
    let status_label_hwnd = GetDlgItem(hwnd, IDC_SERVER_STATUS as i32);
    let conn_hwnd = GetDlgItem(hwnd, IDC_CONNECT as i32);
    if st.auth_failure_notified {
        set_status_dot_color(hwnd, C_RED);
        let _ = SetWindowTextW(status_label_hwnd, &hstring("Pair again required"));
        restore_pair_idle_controls(hwnd);
        ShowWindow(status_hwnd, SW_SHOW);
        return LRESULT(0);
    }
    if connected {
        set_status_dot_color(hwnd, C_GREEN);
        let _ = SetWindowTextW(
            status_label_hwnd,
            &hstring(if paired { "Paired" } else { "Connected" }),
        );
        ShowWindow(conn_hwnd, SW_HIDE);
        ShowWindow(status_hwnd, SW_SHOW);
    } else {
        set_status_dot_color(hwnd, C_RED);
        let _ = SetWindowTextW(
            status_label_hwnd,
            &hstring(if paired { "Paired" } else { "Offline" }),
        );
        EnableWindow(conn_hwnd, true);
        ShowWindow(conn_hwnd, SW_HIDE);
        ShowWindow(status_hwnd, SW_SHOW);
    }
    InvalidateRect(status_hwnd, None, TRUE);
    LRESULT(0)
}

unsafe fn on_app_auth_failed(hwnd: HWND) -> LRESULT {
    let st = stmut(hwnd);
    if st.auth_failure_notified {
        return LRESULT(0);
    }
    st.auth_failure_notified = true;
    if st.sync_engine.is_some() {
        st.sync_engine = None;
    }
    st.connected = false;
    logs::append("WebDAV authentication failed. Automatic sync paused; pair again to continue.");
    let msg = Box::new(
        "WebDAV credentials are invalid. Automatic sync paused; pair again to continue."
            .to_string(),
    );
    PostMessageW(
        hwnd,
        WM_APP_LOG,
        WPARAM(0),
        LPARAM(Box::into_raw(msg) as isize),
    )
    .ok();
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        &hstring("Pair again required"),
    );
    restore_pair_idle_controls(hwnd);
    set_status_dot_color(hwnd, C_RED);
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), None, TRUE);
    msgbox(
        hwnd,
        "WebDAV credentials are invalid. Pair again to continue syncing.",
        "Credentials Invalid",
    );
    LRESULT(0)
}

unsafe fn on_app_pair_result(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 != 1 {
        let err = Box::from_raw(lp.0 as *mut PairError);
        if err.pair_id != stmut(hwnd).pair_id {
            return LRESULT(0);
        }
        let st = stmut(hwnd);
        st.pair_cancel = None;
        let qr_hwnd = st.pair_qr_hwnd;
        if !qr_hwnd.0.is_null() && IsWindow(qr_hwnd).as_bool() {
            DestroyWindow(qr_hwnd).ok();
        }
        restore_pair_idle_controls(hwnd);
        if err.message.is_empty() {
            restore_server_status_after_pair_cancel(hwnd);
            return LRESULT(0);
        }
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
            &hstring("Pair failed"),
        );
        set_status_dot_color(hwnd, C_RED);
        msgbox(hwnd, &err.message, "Pair Device");
        return LRESULT(0);
    }

    let pair = Box::from_raw(lp.0 as *mut PairResult);
    if pair.pair_id != stmut(hwnd).pair_id {
        return LRESULT(0);
    }
    let st = stmut(hwnd);
    st.pair_cancel = None;
    let qr_hwnd = st.pair_qr_hwnd;
    if !qr_hwnd.0.is_null() && IsWindow(qr_hwnd).as_bool() {
        DestroyWindow(qr_hwnd).ok();
    }
    restore_pair_idle_controls(hwnd);
    match secret::encrypt(&pair.device_token) {
        Ok(enc) => st.config.device_token_enc = enc,
        Err(e) => {
            msgbox(
                hwnd,
                &format!("Device token encrypt error: {e}"),
                "Pair Device",
            );
            return LRESULT(0);
        }
    }
    st.config.webdav_url = pair.webdav_url.clone();
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_URL as i32), &hstring(&pair.webdav_url));
    st.config.username = pair.username.clone();
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_USERNAME as i32),
        &hstring(&pair.username),
    );
    match secret::encrypt(&pair.password) {
        Ok(enc) => {
            st.config.password_enc = enc;
            st.password_plain = pair.password.clone();
            let _ = SetWindowTextW(
                GetDlgItem(hwnd, IDC_PASSWORD as i32),
                &hstring(&pair.password),
            );
        }
        Err(e) => {
            msgbox(
                hwnd,
                &format!("WebDAV password encrypt error: {e}"),
                "Pair Device",
            );
            return LRESULT(0);
        }
    }
    st.config.remote_folder = pair.remote_folder.clone();
    st.remote_folder_from_xd = false;
    st.auth_failure_notified = false;
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
        &hstring(&pair.remote_folder),
    );
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_DEST_LABEL as i32),
        &hstring("Approved folder"),
    );
    st.config.credential_profile_id = pair.credential_profile_id;
    st.config.credential_version = pair.credential_version;
    if let Err(e) = crate::config::save(&st.config) {
        msgbox(
            hwnd,
            &format!("Pairing succeeded but save failed: {e}"),
            "Pair Device",
        );
        return LRESULT(0);
    }
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        &hstring("Paired"),
    );
    set_status_dot_color(hwnd, C_GREEN);
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), None, TRUE);
    apply_server_readonly(hwnd);
    start_connection_check(hwnd);
    msgbox(hwnd, "Device paired and saved.", "Pair Device");
    LRESULT(0)
}

unsafe fn cancel_pairing_from_popup(parent: HWND) {
    if parent.0.is_null() || !IsWindow(parent).as_bool() {
        return;
    }
    let parent_state = state_ptr(parent);
    if parent_state.is_null() {
        return;
    }
    if let Some(cancel) = &(*parent_state).pair_cancel {
        cancel.store(true, Ordering::Relaxed);
        restore_pair_idle_controls(parent);
        restore_server_status_after_pair_cancel(parent);
    }
}

unsafe fn on_app_pair_started(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let started = Box::from_raw(lp.0 as *mut PairStarted);
    if started.pair_id != stmut(hwnd).pair_id {
        return LRESULT(0);
    }
    update_pair_qr_window(hwnd, &started.code, &started.approve_url);
    LRESULT(0)
}

unsafe fn on_app_update(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 == 1 {
        return LRESULT(0);
    }
    let url = Box::from_raw(lp.0 as *mut String);
    stmut(hwnd).update_url = Some(*url);
    ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), None, TRUE);
    LRESULT(0)
}

unsafe fn on_app_remote_folder(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let detected = Box::from_raw(lp.0 as *mut crate::xd::DetectedCustomer);
    if is_paired(&stmut(hwnd).config) {
        return LRESULT(0);
    }
    if stmut(hwnd).config.remote_folder.trim().is_empty() {
        let st = stmut(hwnd);
        st.config.remote_folder = detected.folder.clone();
        st.detected_customer = non_empty(detected.customer.clone());
        st.remote_folder_from_xd = true;
        let display = destination_display_text(
            &st.config,
            st.remote_folder_from_xd,
            st.detected_customer.as_deref(),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&display),
        );
        update_server_tooltip(hwnd);
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
    }
    LRESULT(0)
}

unsafe fn on_tray(hwnd: HWND, lp: LPARAM) -> LRESULT {
    match (lp.0 & 0xFFFF) as u32 {
        WM_LBUTTONDBLCLK => {
            ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        WM_RBUTTONUP => tray::show_tray_menu(hwnd),
        _ => {}
    }
    LRESULT(0)
}
