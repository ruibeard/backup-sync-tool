// ── on_create ─────────────────────────────────────────────────────────────────
unsafe fn on_create(hwnd: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);

    let hfont = mkfont("Segoe UI", 12, FW_NORMAL.0 as i32);
    let hfont_hdr = mkfont("Segoe UI", 10, FW_SEMIBOLD.0 as i32);
    let hfont_b = mkfont("Segoe UI", 12, FW_SEMIBOLD.0 as i32);
    let hfont_small = mkfont("Segoe UI", 9, FW_NORMAL.0 as i32);
    let hfont_activity = mkfont("Segoe UI", 8, FW_NORMAL.0 as i32);
    let hfont_btn = mkfont("Segoe UI", 11, FW_NORMAL.0 as i32);
    let hfont_link = mkfont_underline("Segoe UI", 9, FW_NORMAL.0 as i32);

    let mut cfg = crate::config::load();
    let remote_folder_from_xd = false;
    if cfg.watch_folder.is_empty() {
        if let Some(path) = crate::xd::default_watch_folder() {
            cfg.watch_folder = path;
        }
    }
    let pass = secret::decrypt(&cfg.password_enc).unwrap_or_default();
    let sync_configured = is_sync_configured(&cfg, &pass);

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
        sync_last_failed: 0,
        sync_started_at: None,
        sync_anim_frame: 0,
        sync_icon: HICON(std::ptr::null_mut()),
        sync_icon_rect: RECT::default(),
        remote_folder_from_xd,
        detected_customer: None,
        server_tooltip: HWND(std::ptr::null_mut()),
        server_tooltip_text: Vec::new(),
        status_dot_color: C_RED,
        server_status_rect: RECT::default(),
        status_strip_rect: RECT::default(),
        status_strip_display: String::new(),
        status_strip_secondary: String::new(),
        activity_list_rect: RECT::default(),
        dest_path_rect: RECT::default(),
        sync_footer_rect: RECT::default(),
        sync_footer_busy: false,
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        hfont_activity,
        hfont_btn,
        hfont_link,
        br_win: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_status_strip: CreateSolidBrush(COLORREF(C_STATUS_BG)),
        br_path_box: CreateSolidBrush(COLORREF(C_DEST_PATH_BG)),
        br_footer_idle: CreateSolidBrush(COLORREF(C_FOOTER_IDLE_BG)),
        br_footer_busy: CreateSolidBrush(COLORREF(C_STATUS_BG)),
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
        activity_rows: Vec::new(),
        activity_show_empty: true,
        failed_upload_paths: Vec::new(),
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
        hfont_btn,
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

    if sync_configured {
        if let Err(err) = restart_sync_engine(hwnd) {
            let msg = format!("Sync start failed: {err}");
            logs::append(&msg);
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
        let cfg2 = cfg.clone();
        let pass2 = pass.clone();
        set_status_strip_text(hwnd, "Connecting");
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
    hf_btn: HFONT,
    hf_link: HFONT,
) {
    let st = &mut *state_ptr(hwnd);
    let mut y = CONTENT_TOP_PAD;

    // ── STATUS STRIP + SERVER ───────────────────────────────────────────────────
    {
        st.status_strip_rect = RECT {
            left: M,
            top: y,
            right: WIN_W - M,
            bottom: y + STATUS_STRIP_H,
        };
        let dot_size = 10i32;
        let dot_x = M + STATUS_ACCENT_W + 6;
        let dot_y = y + (STATUS_STRIP_H - dot_size) / 2;
        let text_x = dot_x + dot_size + 8;
        let status_initial = if is_paired(cfg) {
            "Connected".to_string()
        } else {
            "Not paired".to_string()
        };
        st.status_strip_display = status_initial;
        st.status_strip_secondary.clear();
        mkstatic(
            hwnd,
            hi,
            IDC_SERVER_STATUS,
            "",
            text_x,
            y,
            1,
            1,
            hf_b,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_SERVER_STATUS as i32), SW_HIDE);
        st.server_status_rect = RECT {
            left: dot_x,
            top: dot_y,
            right: dot_x + dot_size,
            bottom: dot_y + dot_size,
        };
        install_server_tooltip(hwnd, hi);
        y += STATUS_STRIP_H + GAP;

        let pair_x = WIN_W - M - ACTION_BTN_W;
        mkstatic(
            hwnd,
            hi,
            IDC_SERVER_HDR,
            "SERVER",
            M,
            y,
            90,
            ACTION_BTN_H,
            hf_hdr,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_SERVER_URL_LABEL,
            &server_display_text(cfg),
            M + 95,
            y,
            pair_x - M - 95 - PAD,
            ACTION_BTN_H,
            hf_small,
            SS_RIGHT,
        );
        let pair_label = if is_paired(cfg) {
            "Reconnect"
        } else {
            "Connect"
        };
        mkbtn_grey(
            hwnd,
            hi,
            IDC_PAIR_DEVICE,
            pair_label,
            pair_x,
            y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
        y += ACTION_BTN_H + 8;
    }

    // ── FOLDERS ───────────────────────────────────────────────────────────────
    {
        let browse_x = M + INNER_W - ACTION_BTN_W;
        let open_x = browse_x - PAD - ACTION_BTN_W;
        let inp_w = INNER_W - FOLDER_ACTIONS_W - PAD;

        mkstatic(
            hwnd,
            hi,
            IDC_ORIGIN_LABEL,
            "Backup folder on this PC",
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
        mkbtn_grey(
            hwnd,
            hi,
            IDC_OPEN_LOCAL_FOLDER,
            "Open",
            open_x,
            y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
        mkbtn_grey(
            hwnd,
            hi,
            IDC_BROWSE_LOCAL,
            "Browse",
            browse_x,
            y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
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
                "Server destination"
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
        st.dest_path_rect = RECT {
            left: M,
            top: y,
            right: M + INNER_W,
            bottom: y + DEST_PATH_H,
        };
        mkstatic(
            hwnd,
            hi,
            IDC_REMOTE_FOLDER,
            &destination_text,
            M + 10,
            y + 7,
            INNER_W - 20,
            DEST_PATH_H - 14,
            hf_small,
        );
        y += DEST_PATH_H + SECT;

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
        st.activity_list_rect = RECT {
            left: M,
            top: y,
            right: M + INNER_W,
            bottom: y + lb_h,
        };
        mklb(hwnd, hi, IDC_ACTIVITY_LIST, M + 1, y + 1, INNER_W - 2, lb_h - 2, hf_small);
        refresh_activity_listbox(hwnd);
        y += lb_h;
        st.post_list_gap = PAD;
        y += PAD;

        st.sync_row_h = SYNC_FOOTER_H;
        st.sync_footer_rect = RECT {
            left: M,
            top: y,
            right: M + INNER_W,
            bottom: y + SYNC_FOOTER_H,
        };
        let footer_pad_x = 10;
        let footer_pad_y = 8;
        let retry_btn_x = M + INNER_W - footer_pad_x - ACTION_BTN_W;
        mkbtn_grey(
            hwnd,
            hi,
            IDC_RETRY_FAILED,
            "Retry failed",
            retry_btn_x,
            y + footer_pad_y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_RETRY_FAILED as i32), SW_HIDE);
        mkstatic(
            hwnd,
            hi,
            IDC_SYNC_STATUS,
            "All synced",
            M + footer_pad_x,
            y + footer_pad_y,
            retry_btn_x - M - footer_pad_x - PAD,
            LBL_H,
            hf_small,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_SYNC_ETA,
            "",
            M + INNER_W - footer_pad_x - 76,
            y + footer_pad_y,
            76,
            LBL_H,
            hf_small,
            SS_RIGHT,
        );
        let prog = mkprogress(
            hwnd,
            hi,
            IDC_SYNC_PROGRESS,
            M + footer_pad_x,
            y + footer_pad_y + LBL_H + 6,
            INNER_W - footer_pad_x * 2,
            8,
        );
        SendMessageW(prog, PBM_SETBARCOLOR, WPARAM(0), LPARAM(C_BLUE as isize));
        SendMessageW(prog, PBM_SETBKCOLOR, WPARAM(0), LPARAM(C_PROGRESS_TRACK as isize));
        ShowWindow(prog, SW_HIDE);
        y += SYNC_FOOTER_H;
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
        let check_y = y + (row_h - 18) / 2;
        let startup_x = M;
        let startup_w = 126i32;
        let two_way_x = startup_x + startup_w + 12;
        let two_way_w = M + INNER_W - two_way_x;

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
        mkcheck(
            hwnd,
            hi,
            IDC_SYNC_REMOTE,
            "Sync from server",
            two_way_x,
            check_y,
            two_way_w,
            18,
            hf_small,
            cfg.sync_remote_changes,
        );

        y += row_h;

        let footer_y = y + 2;
        let footer_btn_y = footer_y + (LBL_H - ACTION_BTN_H) / 2;
        let version_w = 72i32;
        let author_w = 100i32;
        let version_x = M;
        let github_btn_x = version_x + version_w + PAD;
        let author_x = M + INNER_W - author_w;
        let update_btn_x = author_x - ACTION_BTN_W - PAD;
        let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));

        mklink(
            hwnd,
            hi,
            IDC_REPO,
            ver_label,
            version_x,
            footer_y,
            version_w,
            LBL_H,
            hf_link,
        );

        mkbtn(
            hwnd,
            hi,
            IDC_GITHUB,
            "",
            github_btn_x,
            footer_btn_y,
            GITHUB_BTN_SIZE,
            GITHUB_BTN_SIZE,
            hf_btn,
        );

        mkbtn(
            hwnd,
            hi,
            IDC_UPDATE_LINK,
            "Update",
            update_btn_x,
            footer_btn_y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);

        let author_h = LBL_H - 2;
        mkstatic_align(
            hwnd,
            hi,
            IDC_AUTHOR,
            "Rui Almeida",
            author_x,
            footer_y,
            author_w,
            author_h,
            hf_link,
            SS_RIGHT | SS_NOTIFY,
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
        WINDOW_EX_STYLE::default(),
        w!("LISTBOX"),
        w!(""),
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WINDOW_STYLE(
                LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | LBS_OWNERDRAWVARIABLE as u32,
            ),
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

    for target_id in [
        IDC_SERVER_HDR,
        IDC_SERVER_STATUS,
        IDC_SERVER_URL_LABEL,
        IDC_PAIR_DEVICE,
    ] {
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

fn server_display_text(cfg: &Config) -> String {
    if cfg.webdav_url.trim().is_empty() {
        "Server not configured".to_string()
    } else {
        cfg.webdav_url
            .trim()
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string()
    }
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
    for target_id in [
        IDC_SERVER_HDR,
        IDC_SERVER_STATUS,
        IDC_SERVER_URL_LABEL,
        IDC_PAIR_DEVICE,
    ] {
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
