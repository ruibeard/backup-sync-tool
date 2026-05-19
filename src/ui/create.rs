// ── on_create ─────────────────────────────────────────────────────────────────
unsafe fn on_create(hwnd: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);

    let hfont = mkfont("Segoe UI", 12, FW_NORMAL.0 as i32);
    let hfont_hdr = mkfont("Segoe UI", 10, FW_SEMIBOLD.0 as i32);
    let hfont_b = mkfont("Segoe UI", 12, FW_SEMIBOLD.0 as i32);
    let hfont_small = mkfont("Segoe UI", 9, FW_NORMAL.0 as i32);
    let hfont_link = mkfont_underline("Segoe UI", 9, FW_NORMAL.0 as i32);

    let mut cfg = crate::config::load();
    let remote_folder_from_xd = false;
    if cfg.watch_folder.is_empty() {
        if let Some(path) = crate::xd::default_watch_folder() {
            cfg.watch_folder = path;
        }
    }
    let pass = secret::decrypt(&cfg.password_enc).unwrap_or_default();
    let sync_configured = !cfg.watch_folder.is_empty()
        && !cfg.webdav_url.is_empty()
        && !cfg.username.is_empty()
        && !pass.is_empty()
        && !cfg.remote_folder.is_empty();

    let state = Box::new(WndState {
        config: cfg.clone(),
        password_plain: pass.clone(),
        sync_engine: None,
        update_url: None,
        connected: false,
        sync_status_text: if sync_configured {
            "Starting...".to_string()
        } else {
            "Not configured".to_string()
        },
        sync_status_state: if sync_configured {
            crate::sync::ActivityState::Checking as usize
        } else {
            crate::sync::ActivityState::Idle as usize
        },
        sync_progress_done: 0,
        sync_progress_total: 0,
        sync_started_at: None,
        sync_anim_frame: 0,
        sync_icon: HICON(std::ptr::null_mut()),
        sync_icon_rect: RECT::default(),
        remote_folder_from_xd,
        detected_customer: None,
        server_tooltip: HWND(std::ptr::null_mut()),
        server_tooltip_text: Vec::new(),
        status_dot_color: C_RED,
        status_ok_icon: load_imageres_icon_resource(106),
        status_warn_icon: load_stock_icon(SIID_WARNING, false),
        status_error_icon: load_stock_icon(SIID_ERROR, false),
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        hfont_link,
        br_win: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_sect: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_input: CreateSolidBrush(COLORREF(C_INPUT_BG)),
        focused_edit: 0,
        dividers: Vec::new(),
        activity_list_top: 0,
        activity_list_h: 0,
        post_list_gap: 0,
        sync_row_h: 0,
        post_sync_sect: 0,
        bottom_bar_h: 0,
        divider_activity_idx: 0,
        min_client_h: 0,
        pair_qr_hwnd: HWND(std::ptr::null_mut()),
        pair_cancel: None,
        pair_id: 0,
        auth_failure_notified: false,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    build_ui(
        hwnd,
        hi,
        &cfg,
        &pass,
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        hfont_link,
    );
    apply_server_readonly(hwnd);

    let hicon = LoadIconW(hi, w!("APP_ICON_IDLE"))
        .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(hicon.0 as isize));
    SendMessageW(
        hwnd,
        WM_SETICON,
        WPARAM(ICON_SMALL as usize),
        LPARAM(hicon.0 as isize),
    );
    tray::add_tray_icon(hwnd, hicon);

    let raw = hwnd.0 as isize;
    let log: crate::sync::LogFn = Arc::new(move |m: String| {
        logs::append(&m);
        let s = Box::new(m);
        unsafe {
            PostMessageW(
                HWND(raw as *mut _),
                WM_APP_LOG,
                WPARAM(0),
                LPARAM(Box::into_raw(s) as isize),
            )
            .ok();
        }
    });
    let activity: crate::sync::ActivityFn = Arc::new(move |info| unsafe {
        PostMessageW(
            HWND(raw as *mut _),
            WM_APP_SYNC_ACTIVITY,
            WPARAM(info.state as usize),
            LPARAM(Box::into_raw(Box::new((info.completed, info.total))) as isize),
        )
        .ok();
    });
    let auth_failed: crate::sync::AuthFailedFn = Arc::new(move || unsafe {
        PostMessageW(HWND(raw as *mut _), WM_APP_AUTH_FAILED, WPARAM(0), LPARAM(0)).ok();
    });

    if sync_configured {
        match crate::sync::SyncEngine::start(
            cfg.clone(),
            pass.clone(),
            log.clone(),
            activity.clone(),
            auth_failed.clone(),
        ) {
            Ok(engine) => stmut(hwnd).sync_engine = Some(engine),
            Err(err) => {
                let msg = Box::new(format!("Sync start failed: {err}"));
                PostMessageW(
                    HWND(raw as *mut _),
                    WM_APP_LOG,
                    WPARAM(0),
                    LPARAM(Box::into_raw(msg) as isize),
                )
                .ok();
            }
        }
    }

    if !is_paired(&cfg)
        && (cfg.remote_folder.trim().is_empty() || is_root_remote_folder(&cfg.remote_folder))
    {
        std::thread::spawn(move || {
            if let Some(detected) = crate::xd::detect_customer_hint() {
                unsafe {
                    PostMessageW(
                        HWND(raw as *mut _),
                        WM_APP_REMOTE_FOLDER,
                        WPARAM(0),
                        LPARAM(Box::into_raw(Box::new(detected)) as isize),
                    )
                    .ok();
                }
            }
        });
    }

    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_HIDE);
        let cfg2 = cfg.clone();
        let pass2 = pass.clone();
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
            &hstring("Connecting"),
        );
        std::thread::spawn(move || {
            let ok = crate::webdav::test_connection(&cfg2, &pass2).is_ok();
            PostMessageW(
                HWND(raw as *mut _),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        });
    }

    std::thread::spawn(
        move || match crate::updater::check(env!("CARGO_PKG_VERSION")) {
            crate::updater::CheckResult::UpdateAvailable(info) => {
                crate::logs::append(&format!("Update available: v{}", info.version));
                let url = Box::new(info.url);
                PostMessageW(
                    HWND(raw as *mut _),
                    WM_APP_UPDATE,
                    WPARAM(0),
                    LPARAM(Box::into_raw(url) as isize),
                )
                .ok();
            }
            crate::updater::CheckResult::UpToDate => {}
            crate::updater::CheckResult::Error(e) => {
                crate::logs::append(&format!("Update check error: {e}"));
            }
        },
    );
}

// ── build_ui ──────────────────────────────────────────────────────────────────
unsafe fn build_ui(
    hwnd: HWND,
    hi: HINSTANCE,
    cfg: &Config,
    pass: &str,
    hf: HFONT,
    hf_hdr: HFONT,
    hf_b: HFONT,
    hf_small: HFONT,
    hf_link: HFONT,
) {
    let st = &mut *state_ptr(hwnd);
    let mut y = M + 4;

    // ── SERVER ────────────────────────────────────────────────────────────────
    {
        let status_w = 16i32;
        let pair_x = M + INNER_W - PAIR_BTN_W;
        let server_status_w = SERVER_STATUS_W;
        let server_status_x = pair_x - PAD - server_status_w;
        let status_x = server_status_x - status_w - 6;

        let hdr_toggle_w = 90i32;
        mkstatic(
            hwnd,
            hi,
            IDC_SERVER_HDR,
            "SERVER",
            M,
            y,
            hdr_toggle_w,
            HDR_H,
            hf_hdr,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_SERVER_STATUS,
            "Not connected",
            server_status_x,
            y,
            server_status_w,
            LBL_H,
            hf_small,
            SS_RIGHT,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_STATUS_TEXT,
            "",
            status_x,
            y,
            status_w,
            LBL_H,
            hf_small,
            SS_ICON,
        );
        set_status_icon(hwnd, C_RED);
        mkbtn_grey(
            hwnd,
            hi,
            IDC_PAIR_DEVICE,
            "Pair",
            pair_x,
            y + (HDR_H - SMALL_BTN_H) / 2,
            PAIR_BTN_W,
            SMALL_BTN_H,
            hf_small,
        );
        install_server_tooltip(hwnd, hi);
        y += HDR_H + PAD;
        st.dividers.push(y - SECT / 2);
    }

    // ── FOLDERS ───────────────────────────────────────────────────────────────
    {
        let open_x = M + INNER_W - BROWSE_W;
        let browse_x = open_x - PAD - BROWSE_W;
        let inp_w = INNER_W - FOLDER_ACTIONS_W - PAD;

        mkstatic(
            hwnd,
            hi,
            IDC_ORIGIN_LABEL,
            "Local backup folder",
            M,
            y,
            INNER_W,
            LBL_H,
            hf_small,
        );
        y += LBL_H + 4;
        mkedit_cue(
            hwnd,
            hi,
            IDC_WATCH_FOLDER,
            &cfg.watch_folder,
            "C:\\XDSoftware\\backups",
            M,
            y,
            inp_w,
            hf,
        );
        let browse_btn = mkiconbtn(
            hwnd,
            hi,
            IDC_BROWSE_LOCAL,
            browse_x,
            y,
            34,
            INP_H,
        );
        set_button_icon(browse_btn, load_stock_icon(SIID_FOLDER, false));
        let open_btn = mkiconbtn(
            hwnd,
            hi,
            IDC_OPEN_LOCAL_FOLDER,
            open_x,
            y,
            34,
            INP_H,
        );
        set_button_icon(open_btn, load_stock_icon(SIID_FOLDEROPEN, true));
        y += INP_H + GAP;

        let destination_text = destination_display_text(
            cfg,
            st.remote_folder_from_xd,
            st.detected_customer.as_deref(),
        );

        mkstatic(
            hwnd,
            hi,
            IDC_DEST_LABEL,
            if is_paired(cfg) {
                "Approved folder"
            } else {
                "Destination folder"
            },
            M,
            y,
            150,
            LBL_H,
            hf_small,
        );
        mkstatic(
            hwnd,
            hi,
            IDC_DEST_CREATED,
            "Created on server",
            M + 118,
            y,
            120,
            LBL_H,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
        y += LBL_H + 4;
        mkstatic(
            hwnd,
            hi,
            IDC_REMOTE_FOLDER,
            &destination_text,
            M,
            y,
            INNER_W,
            INP_H,
            hf,
        );
        y += INP_H + SECT;

        st.dividers.push(y - SECT / 2);
    }

    // ── RECENT ACTIVITY ───────────────────────────────────────────────────────
    {
        mkstatic(
            hwnd,
            hi,
            IDC_ACTIVITY_HDR,
            "RECENT ACTIVITY",
            M,
            y,
            INNER_W,
            HDR_H,
            hf_hdr,
        );
        y += HDR_H + PAD;

        let lb_h = 140i32;
        st.activity_list_top = y;
        st.activity_list_h = lb_h;
        mklb(hwnd, hi, IDC_ACTIVITY_LIST, M, y, INNER_W, lb_h, hf_small);
        y += lb_h;
        st.post_list_gap = PAD;
        y += PAD;

        // Sync status row (icon + text + progress) below activity list
        let sync_icon_w = 16i32;
        let sync_gap = 8i32;
        let progress_h = 10;
        let sync_row_h = progress_h + 4;
        st.sync_row_h = sync_row_h;

        // Store icon rect for WM_PAINT
        st.sync_icon_rect = RECT {
            left: M,
            top: y + (sync_row_h - sync_icon_w) / 2,
            right: M + sync_icon_w,
            bottom: y + (sync_row_h - sync_icon_w) / 2 + sync_icon_w,
        };

        let initial_sync_configured = !cfg.watch_folder.is_empty()
            && !cfg.webdav_url.is_empty()
            && !cfg.username.is_empty()
            && !pass.is_empty()
            && !cfg.remote_folder.is_empty();
        let initial_icon = if initial_sync_configured {
            LoadIconW(hi, w!("APP_ICON_SYNCING")).unwrap_or_default()
        } else {
            LoadIconW(hi, w!("APP_ICON_IDLE")).unwrap_or_default()
        };
        st.sync_icon = initial_icon;

        let status_x = M + sync_icon_w + sync_gap;
        let status_w = 180i32;
        mkstatic_align(
            hwnd,
            hi,
            IDC_SYNC_STATUS,
            &st.sync_status_text,
            status_x,
            y + (sync_row_h - LBL_H) / 2,
            status_w,
            LBL_H,
            hf_small,
            SS_LEFT,
        );

        // Progress bar to the right of status
        let progress_x = status_x + status_w + sync_gap;
        let progress_w = INNER_W - (progress_x - M);
        mkprogress(
            hwnd,
            hi,
            IDC_SYNC_PROGRESS,
            progress_x,
            y + (sync_row_h - progress_h) / 2,
            progress_w,
            progress_h,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32), SW_HIDE);

        y += sync_row_h;
        st.post_sync_sect = SECT;
        y += SECT;

        st.divider_activity_idx = st.dividers.len();
        st.dividers.push(y - SECT / 2);
    }

    // ── BOTTOM BAR ────────────────────────────────────────────────────────────
    // Row 1: version + github icon + update + checkboxes + save
    // Row 2: author credit
    {
        let row_h = BTN_H;
        let button_y = y + (row_h - BTN_H) / 2;
        let check_y = y + (row_h - 18) / 2;
        let save_w = 64i32;
        let save_x = M + INNER_W - save_w;
        let startup_x = M;
        let startup_w = 126i32;
        let two_way_x = startup_x + startup_w + 12;
        let two_way_icon_w = 18i32;
        let two_way_check_x = two_way_x + two_way_icon_w;
        let two_way_w = save_x - two_way_x - 12;

        mkcheck(
            hwnd,
            hi,
            IDC_START_WINDOWS,
            "Start with Windows",
            startup_x,
            check_y,
            startup_w,
            18,
            hf_small,
            cfg.start_with_windows,
        );
        let remote_icon = mkstatic_align(
            hwnd,
            hi,
            IDC_SYNC_REMOTE_ICON,
            "",
            two_way_x,
            check_y + 1,
            16,
            16,
            hf_small,
            SS_ICON,
        );
        set_static_icon(remote_icon, load_stock_icon(SIID_SERVER, false));

        mkcheck(
            hwnd,
            hi,
            IDC_SYNC_REMOTE,
            "Download from server",
            two_way_check_x,
            check_y,
            two_way_w - two_way_icon_w,
            18,
            hf_small,
            cfg.sync_remote_changes,
        );

        mkbtn_blue(
            hwnd, hi, IDC_SAVE, "Save", save_x, button_y, save_w, BTN_H, hf_b,
        );

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
        let update_btn_y = footer_y + (footer_h - update_btn_h) / 2;
        let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));

        mklink(
            hwnd, hi, IDC_REPO, ver_label, version_x, footer_y, version_w, footer_h, hf_link,
        );

        // GitHub icon button (owner-drawn, draws GitHub octocat-like icon)
        mkbtn(
            hwnd,
            hi,
            IDC_GITHUB,
            "",
            github_btn_x,
            footer_y,
            github_btn_w,
            footer_h,
            hf_small,
        );

        let update_btn = mkiconbtn(
            hwnd,
            hi,
            IDC_UPDATE_LINK,
            update_btn_x,
            update_btn_y,
            update_btn_w,
            update_btn_h,
        );
        set_button_icon(update_btn, load_stock_icon(SIID_SOFTWARE, false));
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);

        // Author credit row
        let author_h = LBL_H - 2;
        let author_y = footer_y;
        mklink(
            hwnd,
            hi,
            IDC_AUTHOR,
            "Rui Almeida · ruialmeida.me",
            update_btn_x + update_btn_w + 12,
            author_y,
            200,
            author_h,
            hf_link,
        );
        st.bottom_bar_h = row_h + author_h + 4 + M;
    }

    // Size window to fit content
    st.min_client_h = required_client_height(st);
    let mut wr = RECT::default();
    GetWindowRect(hwnd, &mut wr).ok();
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let dh = (wr.bottom - wr.top) - (cr.bottom - cr.top);
    let dw = (wr.right - wr.left) - (cr.right - cr.left);
    SetWindowPos(
        hwnd,
        None,
        0,
        0,
        WIN_W + dw,
        st.min_client_h + dh,
        SWP_NOMOVE | SWP_NOZORDER,
    )
    .ok();

    layout_main(hwnd);
}

// ── Control helpers ───────────────────────────────────────────────────────────

unsafe fn mkstatic(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

unsafe fn mklink(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_NOTIFY | SS_LEFT),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

unsafe fn mkstatic_align(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
    align: u32,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(align),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

/// Edit control with a cue banner placeholder (no label needed)
unsafe fn mkedit_cue(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    placeholder: &str,
    x: i32,
    y: i32,
    w: i32,
    hf: HFONT,
) -> HWND {
    let c = mkedit_raw(hwnd, hi, id, text, x, y, w, hf);
    // EM_SETCUEBANNER = 0x1501
    let ph: Vec<u16> = placeholder
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    SendMessageW(c, EM_SETCUEBANNER, WPARAM(1), LPARAM(ph.as_ptr() as isize));
    c
}

unsafe fn mkedit_raw(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("EDIT"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        x,
        y,
        w,
        INP_H,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    let _ = SetWindowSubclass(c, Some(edit_sub), id as usize, 0);
    c
}

unsafe fn mkbtn(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

unsafe fn mkiconbtn(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        w!(""),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_ICON as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    )
}
unsafe fn mkbtn_blue(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    mkbtn(hwnd, hi, id, text, x, y, w, h, hf)
}
unsafe fn mkbtn_grey(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    mkbtn(hwnd, hi, id, text, x, y, w, h, hf)
}

unsafe fn mkcheck(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
    checked: bool,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    SendMessageW(
        c,
        BM_SETCHECK,
        WPARAM(if checked { BST_CHECKED.0 as usize } else { 0 }),
        LPARAM(0),
    );
    c
}

unsafe fn mklb(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let c = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("LISTBOX"),
        w!(""),
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WINDOW_STYLE(LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

unsafe fn mkprogress(hwnd: HWND, hi: HINSTANCE, id: u16, x: i32, y: i32, w: i32, h: i32) -> HWND {
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("msctls_progress32"),
        w!(""),
        WS_CHILD | WS_VISIBLE,
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as usize as *mut _),
        hi,
        None,
    );
    SendMessageW(c, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
    c
}

unsafe fn install_server_tooltip(hwnd: HWND, hi: HINSTANCE) {
    let tooltip = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        TOOLTIPS_CLASSW,
        w!(""),
        WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP),
        0,
        0,
        0,
        0,
        hwnd,
        HMENU(std::ptr::null_mut()),
        hi,
        None,
    );
    if tooltip.0.is_null() {
        return;
    }

    let st = stmut(hwnd);
    st.server_tooltip = tooltip;
    st.server_tooltip_text = server_tooltip_text(&st.config)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    for target_id in [IDC_SERVER_HDR, IDC_SERVER_STATUS, IDC_STATUS_TEXT] {
        let target = GetDlgItem(hwnd, target_id as i32);
        if target.0.is_null() {
            continue;
        }
        let mut ti = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd,
            uId: target.0 as usize,
            rect: RECT::default(),
            hinst: hi,
            lpszText: PWSTR(st.server_tooltip_text.as_mut_ptr()),
            lParam: LPARAM(0),
            lpReserved: std::ptr::null_mut(),
        };
        SendMessageW(
            tooltip,
            TTM_ADDTOOLW,
            WPARAM(0),
            LPARAM((&mut ti as *mut TTTOOLINFOW) as isize),
        );
    }
}

fn server_tooltip_text(cfg: &Config) -> String {
    let url = if cfg.webdav_url.trim().is_empty() {
        "not set"
    } else {
        cfg.webdav_url.trim()
    };
    let folder = if cfg.remote_folder.trim().is_empty() {
        "waiting for Laravel approval"
    } else {
        cfg.remote_folder.trim()
    };
    format!("Server: {url}\nDestination: {folder}")
}

fn destination_display_text(
    cfg: &Config,
    remote_folder_from_xd: bool,
    detected_customer: Option<&str>,
) -> String {
    if is_paired(cfg) {
        return cfg.remote_folder.clone();
    }
    if remote_folder_from_xd && !cfg.remote_folder.trim().is_empty() {
        if let Some(customer) = detected_customer.and_then(non_empty_str) {
            return format!("{customer} ({})", cfg.remote_folder);
        }
        return cfg.remote_folder.clone();
    }
    "Waiting for pairing approval".to_string()
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

unsafe fn update_server_tooltip(hwnd: HWND) {
    let st = stmut(hwnd);
    if st.server_tooltip.0.is_null() {
        return;
    }
    st.server_tooltip_text = server_tooltip_text(&st.config)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    for target_id in [IDC_SERVER_HDR, IDC_SERVER_STATUS, IDC_STATUS_TEXT] {
        let target = GetDlgItem(hwnd, target_id as i32);
        if target.0.is_null() {
            continue;
        }
        let mut ti = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd,
            uId: target.0 as usize,
            rect: RECT::default(),
            hinst: HINSTANCE(std::ptr::null_mut()),
            lpszText: PWSTR(st.server_tooltip_text.as_mut_ptr()),
            lParam: LPARAM(0),
            lpReserved: std::ptr::null_mut(),
        };
        SendMessageW(
            st.server_tooltip,
            TTM_UPDATETIPTEXTW,
            WPARAM(0),
            LPARAM((&mut ti as *mut TTTOOLINFOW) as isize),
        );
    }
}
