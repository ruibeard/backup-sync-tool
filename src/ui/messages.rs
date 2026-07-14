// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    apply_activity_log(hwnd, &msg);
    LRESULT(0)
}

unsafe fn on_app_snapshot(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let snapshot = Box::from_raw(lp.0 as *mut crate::app::AppSnapshot);
    let state = match snapshot.work {
        crate::app::WorkState::Scanning => UiSyncState::Checking,
        crate::app::WorkState::Syncing => UiSyncState::Syncing,
        crate::app::WorkState::Idle | crate::app::WorkState::PausedForReconnect => UiSyncState::Idle,
    };
    let new_activity = {
        let st = stmut(hwnd);
        st.connected = snapshot.hub_connected;
        st.sync_status_state = state as usize;
        st.sync_progress_total = snapshot.global_files as usize;
        st.sync_progress_done = snapshot.global_files.saturating_sub(snapshot.need_files) as usize;
        st.sync_status_text = match state {
            UiSyncState::Checking => "Checking...".into(),
            UiSyncState::Syncing => format!("Syncing {} file(s)...", snapshot.need_files),
            UiSyncState::Idle if snapshot.hub_connected => "All synced".into(),
            UiSyncState::Idle => "Hub offline".into(),
        };
        let activity = if snapshot.last_event_id > st.last_event_id {
            snapshot.activity.back().cloned()
        } else {
            None
        };
        st.last_event_id = st.last_event_id.max(snapshot.last_event_id);
        (activity, (st.sync_progress_done, st.sync_progress_total, 0))
    };
    if let Some(line) = new_activity.0 {
        apply_activity_log(hwnd, &line);
    }
    if state == UiSyncState::Checking || state == UiSyncState::Syncing {
        SetTimer(hwnd, IDT_SYNC_ANIM, SYNC_ANIM_MS, None);
    } else {
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
    }
    set_status_strip_connection(hwnd);
    update_sync_footer(hwnd, state as usize, new_activity.1);
    InvalidateRect(hwnd, Some(&stmut(hwnd).bridge_progress_rect), TRUE);
    LRESULT(0)
}

unsafe fn on_timer(hwnd: HWND, wp: WPARAM) -> LRESULT {
    if wp.0 != IDT_SYNC_ANIM {
        return DefWindowProcW(hwnd, WM_TIMER, wp, LPARAM(0));
    }

    let st = stmut(hwnd);
    if st.sync_status_state != UiSyncState::Checking as usize
        && st.sync_status_state != UiSyncState::Syncing as usize
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
    let list = activity_list_hwnd(hwnd);
    if !list.0.is_null() {
        InvalidateRect(list, None, TRUE);
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
    let _ = st.app.send(crate::app::AppCommand::EngineFailed(
        "Syncthing assignment was rejected.".into(),
    ));
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
        "The Syncthing assignment is invalid. Use Reconnect to continue.",
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
            stmut(hwnd)
                .app
                .send(crate::app::AppCommand::CancelPairing)
                .ok();
            restore_server_status_after_pair_cancel(hwnd);
            return LRESULT(0);
        }
        if err.approval_received && is_paired(&stmut(hwnd).config) {
            logs::append(&format!(
                "Approved reconnect could not be activated: {}",
                err.message
            ));
            return on_app_auth_failed(hwnd);
        }
        set_status_strip_text(hwnd, "Pair failed");
        stmut(hwnd)
            .app
            .send(crate::app::AppCommand::PairFailed {
                message: err.message.clone(),
                retryable: true,
            })
            .ok();
        notify_user_status(hwnd, "Pair failed", C_RED, &err.message);
        let _ = SetForegroundWindow(hwnd);
        return LRESULT(0);
    }

    let pair = Box::from_raw(lp.0 as *mut PairResult);
    if pair.pair_id != stmut(hwnd).pair_id {
        return LRESULT(0);
    }
    stmut(hwnd)
        .app
        .send(crate::app::AppCommand::PairApproved)
        .ok();
    let prior_config = stmut(hwnd).config.clone();
    let prior_was_paired = is_paired(&prior_config);
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
    let mut candidate_config = stmut(hwnd).config.clone();
    stmut(hwnd).config = prior_config;

    candidate_config.schema_version = crate::config::CONFIG_SCHEMA_VERSION;
    candidate_config.device_uuid = pair.device_uuid.clone();
    candidate_config.syncthing_device_id = pair.syncthing_device_id.clone();
    candidate_config.syncthing_hub_device_id = pair.syncthing_hub_device_id.clone();
    candidate_config.syncthing_hub_addresses = pair.syncthing_hub_addresses.clone();
    candidate_config.syncthing_folder_id = pair.syncthing_folder_id.clone();
    candidate_config.syncthing_folder_label = pair.syncthing_folder_label.clone();
    candidate_config.server_approved_at = Some(approval_timestamp_now());

    let candidate_config = match crate::config::save_pairing_candidate(candidate_config, &pair.device_token) {
        Ok(config) => config,
        Err(e) => {
            logs::append(&format!("Approved reconnect save failed: {e}"));
            if prior_was_paired {
                return on_app_auth_failed(hwnd);
            }
            notify_user_status(hwnd, "Save failed", C_RED, &e);
            return LRESULT(0);
        }
    };
    {
        let st = stmut(hwnd);
        st.config = candidate_config;
        st.remote_folder_from_xd = false;
        st.auth_failure_notified = false;
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&pair.syncthing_folder_label),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_SERVER_URL_LABEL as i32),
            &hstring(&server_display_text(&st.config)),
        );
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_DEST_LABEL as i32),
            &hstring("Server destination"),
        );
    }
    apply_activity_log(
        hwnd,
        &format!("! Syncthing folder approved: {}", pair.syncthing_folder_label),
    );
    match restart_sync_engine(hwnd) {
        Ok(()) => logs::append("Pairing complete; initial sync started."),
        Err(err) => {
            let msg =
                format!("Paired but sync did not start: {err}. Set the backup folder on this PC.");
            notify_user_status(hwnd, "Sync not started", C_AMBER, &msg);
            apply_server_readonly(hwnd);
            start_connection_check(hwnd);
            let _ = SetForegroundWindow(hwnd);
            return LRESULT(0);
        }
    }
    {
        let st = stmut(hwnd);
        st.sync_status_state = UiSyncState::Checking as usize;
        st.sync_status_text = "Checking...".to_string();
    }
    set_status_strip_connection(hwnd);
    layout_main(hwnd);
    invalidate_bridge(hwnd);
    apply_server_readonly(hwnd);
    start_connection_check(hwnd);
    let _ = SetForegroundWindow(hwnd);
    notify_user(hwnd, "Device paired. Synchronization started.");
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
    let api_base = stmut(hwnd).config.pair_api_base.clone();
    stmut(hwnd)
        .app
        .send(crate::app::AppCommand::PairStarted {
            code: started.code.clone(),
            approve_url: started.approve_url.clone(),
            api_base,
        })
        .ok();
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

unsafe fn on_app_repair_failed(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let error = Box::from_raw(lp.0 as *mut String);
    logs::append(&format!("Installation repair failed: {error}"));
    let st = stmut(hwnd);
    st.repair_required = true;
    let button = GetDlgItem(hwnd, IDC_UPDATE_LINK as i32);
    let _ = SetWindowTextW(button, &hstring("Retry repair"));
    EnableWindow(button, true);
    ShowWindow(button, SW_SHOW);
    notify_user_status(
        hwnd,
        "Repair failed",
        C_RED,
        &format!("{error}\n\nChoose Retry repair to try again."),
    );
    ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    LRESULT(0)
}

unsafe fn on_app_remote_folder(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let detected = Box::from_raw(lp.0 as *mut crate::xd::DetectedCustomer);
    if is_paired(&stmut(hwnd).config) {
        return LRESULT(0);
    }
    // User already Chose a non-XD folder — do not overwrite with licence name.
    if !crate::xd::is_xd_default_watch_folder(&stmut(hwnd).config.watch_folder)
        && !stmut(hwnd).config.watch_folder.trim().is_empty()
    {
        return LRESULT(0);
    }

    let st = stmut(hwnd);
    st.detected_customer = non_empty(detected.customer.clone());
    st.remote_folder_from_xd = true;
    let display = st.detected_customer.clone().unwrap_or(detected.folder.clone());
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
        &hstring(&display),
    );
    invalidate_bridge(hwnd);
    update_server_tooltip(hwnd);
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
