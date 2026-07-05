// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    apply_activity_log(hwnd, &msg);
    LRESULT(0)
}

unsafe fn on_app_sync_activity(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let progress = if lp.0 != 0 {
        *Box::from_raw(lp.0 as *mut (usize, usize, usize, Vec<String>))
    } else {
        (0, 0, 0, Vec::new())
    };
    let failed_paths = progress.3.clone();
    let (icon_name, mut status_text) = match wp.0 {
        x if x == crate::sync::ActivityState::Checking as usize => {
            (w!("APP_ICON_SYNCING"), "Checking...")
        }
        x if x == crate::sync::ActivityState::Syncing as usize => {
            (w!("APP_ICON_SYNCING"), "Syncing...")
        }
        _ if progress.2 > 0 => (
            w!("APP_ICON_SYNCING"),
            "Upload errors",
        ),
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
    st.sync_last_failed = progress.2;
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
        }
    } else {
        st.sync_started_at = None;
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);
        let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
        if !hicon.0.is_null() {
            tray::set_tray_icon_and_tip(hwnd, hicon, "Backup Sync Tool");
        }
    }
    if !is_syncing {
        st.sync_status_text = status_text.to_string();
    }
    let is_idle = wp.0 == crate::sync::ActivityState::Idle as usize;
    if is_idle && progress.2 > 0 {
        apply_sync_batch_failures(hwnd, &failed_paths);
    } else if is_idle && progress.2 == 0 && was_syncing {
        finalize_stuck_upload_rows(hwnd);
        update_retry_failed_button(hwnd);
    }
    update_status_strip_after_sync(hwnd, wp.0, (progress.0, progress.1, progress.2));
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
    }
    InvalidateRect(hwnd, Some(&st.sync_footer_rect), TRUE);
    InvalidateRect(hwnd, Some(&st.bridge_rect), TRUE);
    InvalidateRect(hwnd, Some(&st.bridge_progress_rect), TRUE);
    InvalidateRect(hwnd, Some(&st.activity_list_rect), TRUE);
    let hlb = activity_list_hwnd(hwnd);
    if !hlb.0.is_null() {
        InvalidateRect(hlb, None, TRUE);
    }
    LRESULT(0)
}

unsafe fn on_app_connected(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let connected = wp.0 == 1;
    let st = stmut(hwnd);
    st.connected = connected;
    if st.auth_failure_notified {
        set_status_dot_color(hwnd, C_RED);
        set_status_strip_text(hwnd, "Reconnect required");
        restore_pair_idle_controls(hwnd);
        invalidate_bridge(hwnd);
        return LRESULT(0);
    }
    update_status_strip_from_connection(hwnd);
    invalidate_bridge(hwnd);
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
    restore_pair_idle_controls(hwnd);
    notify_user_status(
        hwnd,
        "Reconnect required",
        C_RED,
        "Credentials invalid. Automatic sync paused; use Reconnect to continue.",
    );
    let _ = SetForegroundWindow(hwnd);
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
        set_status_strip_text(hwnd, "Pair failed");
        notify_user_status(hwnd, "Pair failed", C_RED, &err.message);
        let _ = SetForegroundWindow(hwnd);
        return LRESULT(0);
    }

    let pair = Box::from_raw(lp.0 as *mut PairResult);
    if pair.pair_id != stmut(hwnd).pair_id {
        return LRESULT(0);
    }
    {
        let st = stmut(hwnd);
        st.pair_cancel = None;
        let qr_hwnd = st.pair_qr_hwnd;
        if !qr_hwnd.0.is_null() && IsWindow(qr_hwnd).as_bool() {
            DestroyWindow(qr_hwnd).ok();
        }
        restore_pair_idle_controls(hwnd);
        read_ctrls(hwnd, st);
    }
    if !ensure_or_prompt_watch_folder(hwnd) {
        notify_user_status(
            hwnd,
            "Backup folder required",
            C_AMBER,
            "Pairing was not saved. Choose a backup folder on this PC, then pair again.",
        );
        apply_server_readonly(hwnd);
        let _ = SetForegroundWindow(hwnd);
        return LRESULT(0);
    }
    {
        let st = stmut(hwnd);
        match secret::encrypt(&pair.device_token) {
            Ok(enc) => st.config.device_token_enc = enc,
            Err(e) => {
                notify_user_status(
                    hwnd,
                    "Pair failed",
                    C_RED,
                    &format!("Device token encrypt error: {e}"),
                );
                return LRESULT(0);
            }
        }
        st.config.webdav_url = pair.webdav_url.clone();
        st.config.username = pair.username.clone();
        match secret::encrypt(&pair.password) {
            Ok(enc) => {
                st.config.password_enc = enc;
                st.password_plain = pair.password.clone();
            }
            Err(e) => {
                notify_user_status(
                    hwnd,
                    "Pair failed",
                    C_RED,
                    &format!("WebDAV password encrypt error: {e}"),
                );
                return LRESULT(0);
            }
        }
        st.config.remote_folder = pair.remote_folder.clone();
        st.remote_folder_from_xd = false;
        st.auth_failure_notified = false;
        st.config.server_approved_at = Some(approval_timestamp_now());
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&pair.remote_folder),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_SERVER_URL_LABEL as i32),
            &hstring(&server_display_text(&st.config)),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_DEST_LABEL as i32),
            &hstring("Server destination"),
        );
        st.config.credential_profile_id = pair.credential_profile_id;
        st.config.credential_version = pair.credential_version;
    }
    if let Err(e) = crate::config::save(&stmut(hwnd).config) {
        notify_user_status(
            hwnd,
            "Save failed",
            C_RED,
            &format!("Pairing succeeded but save failed: {e}"),
        );
        return LRESULT(0);
    }
    apply_activity_log(
        hwnd,
        &format!("Server approved destination: {}", pair.remote_folder),
    );
    match restart_sync_engine(hwnd) {
        Ok(()) => logs::append("Pairing complete; initial sync started."),
        Err(err) => {
            let msg = format!(
                "Paired but sync did not start: {err}. Set the backup folder on this PC."
            );
            notify_user_status(hwnd, "Sync not started", C_AMBER, &msg);
            apply_server_readonly(hwnd);
            start_connection_check(hwnd);
            let _ = SetForegroundWindow(hwnd);
            return LRESULT(0);
        }
    }
    {
        let st = stmut(hwnd);
        st.sync_status_state = crate::sync::ActivityState::Checking as usize;
        st.sync_status_text = "Checking...".to_string();
    }
    set_status_strip_connection(hwnd);
    layout_main(hwnd);
    invalidate_bridge(hwnd);
    apply_server_readonly(hwnd);
    start_connection_check(hwnd);
    let _ = SetForegroundWindow(hwnd);
    notify_user(hwnd, "Device paired. Uploading backup folder.");
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
    let auto_install = wp.0 == 1;
    let url = Box::from_raw(lp.0 as *mut String);
    if auto_install {
        notify_user(hwnd, "Update available. Installing automatically...");
        start_update_install(hwnd, *url);
        return LRESULT(0);
    }
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
        invalidate_bridge(hwnd);
        update_server_tooltip(hwnd);
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
