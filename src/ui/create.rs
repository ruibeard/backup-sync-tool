// ── on_create ─────────────────────────────────────────────────────────────────
unsafe fn on_create(hwnd: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut _);

    // Readable pixel-height scale (see FONT_* constants in common.rs)
    let hfont = mkfont_px("Segoe UI", FONT_BODY_PX, FW_NORMAL.0 as i32);
    let hfont_hdr = mkfont_px("Segoe UI", FONT_SECTION_PX, FW_BOLD.0 as i32);
    let hfont_b = mkfont_px("Segoe UI", FONT_EMPHASIS_PX, FW_SEMIBOLD.0 as i32);
    let hfont_small = mkfont_px("Segoe UI", FONT_CAPTION_PX, FW_NORMAL.0 as i32);
    let hfont_activity = mkfont_px("Segoe UI", FONT_BODY_PX, FW_NORMAL.0 as i32);
    let hfont_btn = mkfont_px("Segoe UI", FONT_BTN_PX, FW_NORMAL.0 as i32);
    let hfont_bridge = mkfont_px("Segoe UI", FONT_BTN_SM_PX, FW_NORMAL.0 as i32);
    let hfont_bridge_name = mkfont_px("Segoe UI", FONT_EMPHASIS_PX, FW_SEMIBOLD.0 as i32);
    let hfont_bridge_path = mkfont_px("Segoe UI", FONT_CAPTION_PX, FW_NORMAL.0 as i32);
    let hfont_bridge_mid = mkfont_px("Segoe UI", FONT_EMPHASIS_PX, FW_SEMIBOLD.0 as i32);
    let hfont_bridge_check = mkfont_px("Segoe UI", FONT_BRIDGE_CHECK_PX, FW_SEMIBOLD.0 as i32);
    let hfont_link = mkfont_px_underline("Segoe UI", FONT_LINK_PX, FW_NORMAL.0 as i32);

    let mut cfg = crate::config::load();
    let remote_folder_from_xd = false;
    // Empty or stale path (e.g. old Desktop\Sync) → XD default when present.
    if !watch_folder_is_valid(&cfg.watch_folder) {
        if let Some(path) = crate::xd::default_watch_folder() {
            cfg.watch_folder = path;
        }
    }
    let s3_secret = secret::decrypt(&cfg.s3_secret_enc).unwrap_or_default();
    let sync_configured = is_sync_configured(&cfg, &s3_secret);
    let (bridge_icon_pc, bridge_icon_cloud) = load_bridge_icons(hwnd);

    let state = Box::new(WndState {
        config: cfg.clone(),
        s3_secret_plain: s3_secret.clone(),
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
        remote_folder_from_xd,
        detected_customer: None,
        server_tooltip: HWND(std::ptr::null_mut()),
        server_tooltip_text: Vec::new(),
        status_dot_color: C_RED,
        server_status_rect: RECT::default(),
        status_strip_rect: RECT::default(),
        status_strip_display: String::new(),
        status_subtitle: String::new(),
        bridge_rect: RECT::default(),
        bridge_progress_rect: RECT::default(),
        bridge_sync_head: String::new(),
        bridge_sync_meta: String::new(),
        bridge_conn_label: String::new(),
        bridge_conn_ok: false,
        bridge_btn_y: 0,
        bridge_icon_pc,
        bridge_icon_cloud,
        inner_w: INNER_W,
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
        hfont_bridge,
        hfont_bridge_name,
        hfont_bridge_path,
        hfont_bridge_mid,
        hfont_bridge_check,
        hfont_link,
        br_win: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_path_box: CreateSolidBrush(COLORREF(C_DEST_PATH_BG)),
        br_footer_idle: CreateSolidBrush(COLORREF(C_FOOTER_IDLE_BG)),
        br_footer_busy: CreateSolidBrush(COLORREF(C_STATUS_BG)),
        br_input: CreateSolidBrush(COLORREF(C_INPUT_BG)),
        focused_edit: 0,
        activity_list_top: 0,
        activity_list_h: 0,
        post_list_gap: 0,
        sync_row_h: 0,
        post_sync_sect: 0,
        bottom_bar_h: 0,
        min_client_h: 0,
        footer_panel_rect: RECT::default(),
        pair_qr_hwnd: HWND(std::ptr::null_mut()),
        pair_cancel: None,
        restore_cancel: None,
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
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        hfont_btn,
        hfont_bridge,
        hfont_link,
    );
    apply_server_readonly(hwnd);
    update_pair_button_enabled(hwnd);
    if !is_paired(&cfg) && !cfg.watch_folder.trim().is_empty() {
        if crate::xd::is_xd_default_watch_folder(&cfg.watch_folder) {
            // leave async XD detection below
        } else {
            apply_pairing_folder_hint(hwnd, &cfg.watch_folder);
        }
    }

    let hicon = LoadIconW(hi, w!("APP_ICON_IDLE"))
        .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    SendMessageW(
        hwnd,
        WM_SETICON,
        WPARAM(ICON_BIG as usize),
        LPARAM(hicon.0 as isize),
    );
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

    let watch_for_xd = cfg.watch_folder.clone();
    if !is_paired(&cfg)
        && (watch_for_xd.trim().is_empty()
            || crate::xd::is_xd_default_watch_folder(&watch_for_xd))
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

    if is_sync_configured(&cfg, &s3_secret) {
        let cfg2 = cfg.clone();
        let s3_2 = s3_secret.clone();
        set_status_strip_text(hwnd, "Connecting");
        std::thread::spawn(move || {
            let ok = match crate::transport::build(&cfg2, &s3_2) {
                Ok(t) => t.test_connection().is_ok(),
                Err(_) => false,
            };
            PostMessageW(
                HWND(raw as *mut _),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        });
    }

    let auto_update = cfg.auto_update;
    std::thread::spawn(
        move || match crate::updater::check(env!("CARGO_PKG_VERSION")) {
            crate::updater::CheckResult::UpdateAvailable(info) => {
                crate::logs::append(&format!("Update available: v{}", info.version));
                let url = Box::new(info.url);
                PostMessageW(
                    HWND(raw as *mut _),
                    WM_APP_UPDATE,
                    WPARAM(if auto_update { 1 } else { 0 }),
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
    hf: HFONT,
    hf_hdr: HFONT,
    hf_b: HFONT,
    hf_small: HFONT,
    hf_btn: HFONT,
    hf_bridge: HFONT,
    hf_link: HFONT,
) {
    let st = &mut *state_ptr(hwnd);
    let mut y = CONTENT_TOP_PAD;

    // ── H6 hero bridge (connection + sync in card) ─────────────────────────────
    {
        st.status_strip_rect = RECT::default();
        st.server_status_rect = RECT::default();
        st.status_strip_display.clear();
        st.status_subtitle.clear();
        mkstatic(hwnd, hi, IDC_SERVER_STATUS, "", 0, 0, 1, 1, hf_b);
        ShowWindow(GetDlgItem(hwnd, IDC_SERVER_STATUS as i32), SW_HIDE);
        install_server_tooltip(hwnd, hi);

        y = layout_bridge_section(hwnd, hi, cfg, y, hf_bridge);
        update_bridge_display(hwnd);
    }

    // Hidden legacy controls (still used for read_ctrls / tooltips)
    {
        mkstatic(hwnd, hi, IDC_SERVER_HDR, "SERVER", 0, 0, 1, 1, hf_hdr);
        ShowWindow(GetDlgItem(hwnd, IDC_SERVER_HDR as i32), SW_HIDE);
        mkstatic_align(
            hwnd,
            hi,
            IDC_SERVER_URL_LABEL,
            &server_display_text(cfg),
            0,
            0,
            1,
            1,
            hf_small,
            SS_RIGHT,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_SERVER_URL_LABEL as i32), SW_HIDE);
        mkstatic(
            hwnd,
            hi,
            IDC_ORIGIN_LABEL,
            "Backup folder on this PC",
            0,
            0,
            1,
            1,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_ORIGIN_LABEL as i32), SW_HIDE);
        mkstatic(
            hwnd,
            hi,
            IDC_DEST_LABEL,
            "Server destination",
            0,
            0,
            1,
            1,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_LABEL as i32), SW_HIDE);
        mkedit_cue(
            hwnd,
            hi,
            IDC_WATCH_FOLDER,
            &cfg.watch_folder,
            "C:\\XDSoftware\\backups",
            0,
            0,
            1,
            hf,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32), SW_HIDE);
        let destination_text = destination_display_text(
            cfg,
            st.remote_folder_from_xd,
            st.detected_customer.as_deref(),
        );
        st.dest_path_rect = RECT::default();
        mkstatic(
            hwnd,
            hi,
            IDC_REMOTE_FOLDER,
            &destination_text,
            0,
            0,
            1,
            1,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32), SW_HIDE);
    }

    // ── CONTROL PLANE (which Laravel install to pair with) ───────────────────
    {
        mkstatic(
            hwnd,
            hi,
            IDC_PAIR_API_LABEL,
            "CONTROL PLANE URL",
            M,
            y,
            INNER_W,
            HDR_H,
            hf_hdr,
        );
        y += HDR_H + 4;
        mkedit_cue(
            hwnd,
            hi,
            IDC_PAIR_API_BASE,
            &cfg.pair_api_base,
            "https://backup.rui.cam",
            M,
            y,
            INNER_W,
            hf,
        );
        y += INP_H + SECT;
    }

    // ── RECENT ACTIVITY ───────────────────────────────────────────────────────
    {
        let sub_w = 180;
        mkstatic(
            hwnd,
            hi,
            IDC_ACTIVITY_HDR,
            "RECENT ACTIVITY LOG",
            M,
            y,
            INNER_W - sub_w - PAD,
            HDR_H,
            hf_hdr,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_ACTIVITY_SUBHDR,
            &activity_subhdr_text(),
            M + INNER_W - sub_w,
            y,
            sub_w,
            HDR_H,
            hf_small,
            SS_RIGHT,
        );
        y += HDR_H + PAD;

        let lb_h = MIN_ACTIVITY_LIST_H;
        st.activity_list_top = y;
        st.activity_list_h = lb_h;
        st.activity_list_rect = RECT {
            left: M,
            top: y,
            right: M + INNER_W,
            bottom: y + lb_h,
        };
        mklb(
            hwnd,
            hi,
            IDC_ACTIVITY_LIST,
            M + 1,
            y + 1,
            INNER_W - 2,
            lb_h - 2,
            hf_small,
        );
        refresh_activity_listbox(hwnd);
        y += lb_h;
        st.post_list_gap = PAD;
        y += PAD;

        st.sync_row_h = 0;
        st.sync_footer_rect = RECT {
            left: M,
            top: y,
            right: M + INNER_W,
            bottom: y,
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
        ShowWindow(GetDlgItem(hwnd, IDC_SYNC_STATUS as i32), SW_HIDE);
        ShowWindow(GetDlgItem(hwnd, IDC_SYNC_ETA as i32), SW_HIDE);
        y += SYNC_FOOTER_H;
        st.post_sync_sect = SECT;
        y += SECT;
    }

    // ── BOTTOM BAR ────────────────────────────────────────────────────────────
    // Row 1: version + github icon + update + checkboxes
    // Row 2: author credit
    {
        let row_h = BTN_H;
        let check_h = 22;
        let check_y = y + (row_h - check_h) / 2;
        let startup_x = M;
        let startup_w = 154i32;
        let auto_update_x = startup_x + startup_w + 12;
        let auto_update_w = M + INNER_W - auto_update_x;

        mkcheck(
            hwnd,
            hi,
            IDC_START_WINDOWS,
            "Start with Windows",
            startup_x,
            check_y,
            startup_w,
            check_h,
            hf,
            cfg.start_with_windows,
        );
        mkcheck(
            hwnd,
            hi,
            IDC_AUTO_UPDATE,
            "Auto-update",
            auto_update_x,
            check_y,
            auto_update_w,
            check_h,
            hf,
            cfg.auto_update,
        );

        y += row_h;

        let footer_y = y + 2;
        let meta = footer_meta_layout(footer_y, hf_link, "Rui Almeida");
        let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));

        mklink(
            hwnd,
            hi,
            IDC_REPO,
            ver_label,
            meta.version_x,
            meta.footer_y,
            meta.version_w,
            LBL_H,
            hf_link,
        );

        mkbtn(
            hwnd,
            hi,
            IDC_GITHUB,
            "",
            meta.github_x,
            meta.footer_btn_y,
            GITHUB_BTN_SIZE,
            GITHUB_BTN_SIZE,
            hf_btn,
        );

        mkbtn(
            hwnd,
            hi,
            IDC_UPDATE_LINK,
            "Update",
            meta.update_x,
            meta.footer_btn_y,
            ACTION_BTN_W,
            ACTION_BTN_H,
            hf_btn,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);

        mkstatic_align(
            hwnd,
            hi,
            IDC_AUTHOR,
            "Rui Almeida",
            meta.author_x,
            meta.footer_y,
            meta.author_w,
            LBL_H - 2,
            hf_link,
            SS_RIGHT | SS_NOTIFY,
        );
        st.bottom_bar_h = row_h + (LBL_H - 2) + 4 + M;
    }

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
        IDC_REFRESH_REMOTE,
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

struct FooterMetaLayout {
    version_x: i32,
    version_w: i32,
    github_x: i32,
    update_x: i32,
    author_x: i32,
    author_w: i32,
    footer_y: i32,
    footer_btn_y: i32,
}

unsafe fn font_text_width(hf: HFONT, text: &str) -> i32 {
    let hdc = GetDC(None);
    if hdc.0.is_null() {
        return 72;
    }
    let prev = SelectObject(hdc, hf);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut sz = SIZE::default();
    let _ = gdi::GetTextExtentPoint32W(hdc, &wide, &mut sz);
    SelectObject(hdc, prev);
    ReleaseDC(None, hdc);
    sz.cx.max(0) + 2
}

unsafe fn footer_meta_layout(footer_y: i32, hf_link: HFONT, slug: &str) -> FooterMetaLayout {
    let version_x = M;
    let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));
    let version_w = font_text_width(hf_link, ver_label);
    let github_x = version_x + version_w + META_ICON_GAP;
    let slug_w = font_text_width(hf_link, slug) + 16;
    let author_w = slug_w.max(120).min(INNER_W / 2);
    let author_x = M + INNER_W - author_w;
    let update_x = github_x + GITHUB_BTN_SIZE + META_ICON_GAP;
    let footer_btn_y = footer_y + (LBL_H - ACTION_BTN_H) / 2;
    FooterMetaLayout {
        version_x,
        version_w,
        github_x,
        update_x,
        author_x,
        author_w,
        footer_y,
        footer_btn_y,
    }
}

unsafe fn position_footer_meta(hwnd: HWND, layout: &FooterMetaLayout, _slug: &str) {
    let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));
    SetWindowPos(
        GetDlgItem(hwnd, IDC_REPO as i32),
        None,
        layout.version_x,
        layout.footer_y,
        layout.version_w,
        LBL_H,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_GITHUB as i32),
        None,
        layout.github_x,
        layout.footer_btn_y,
        GITHUB_BTN_SIZE,
        GITHUB_BTN_SIZE,
        SWP_NOZORDER,
    )
    .ok();
    SetWindowPos(
        GetDlgItem(hwnd, IDC_UPDATE_LINK as i32),
        None,
        layout.update_x,
        layout.footer_btn_y,
        ACTION_BTN_W,
        ACTION_BTN_H,
        SWP_NOZORDER,
    )
    .ok();
    let author_h = LBL_H - 2;
    SetWindowPos(
        GetDlgItem(hwnd, IDC_AUTHOR as i32),
        None,
        layout.author_x,
        layout.footer_y,
        layout.author_w,
        author_h,
        SWP_NOZORDER,
    )
    .ok();
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_REPO as i32), &hstring(ver_label));
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_AUTHOR as i32), &hstring("Rui Almeida"));
}

unsafe fn layout_bridge_section(
    hwnd: HWND,
    hi: HINSTANCE,
    cfg: &Config,
    mut y: i32,
    hf_bridge: HFONT,
) -> i32 {
    let st = state_ptr(hwnd);
    let inner_w = (*st).inner_w;
    let pair_label = if is_paired(cfg) {
        "Reconnect Server"
    } else {
        "Connect Server"
    };

    let layout = bridge_layout_at(y, inner_w);
    let show_band = bridge_show_sync_band(&*st);
    let card_h = layout.height;
    let local_folder_valid = watch_folder_is_valid(&cfg.watch_folder);

    (*st).bridge_rect = RECT {
        left: M,
        top: y,
        right: M + inner_w,
        bottom: y + card_h,
    };

    (*st).bridge_btn_y = layout.btn_y;

    let place_btn =
        |hwnd: HWND, hi: HINSTANCE, id: u16, label: &str, x: i32, by: i32, w: i32, h: i32| {
            let existing = GetDlgItem(hwnd, id as i32);
            if existing.0.is_null() {
                mkbtn_grey(hwnd, hi, id, label, x, by, w, h, hf_bridge);
            } else {
                SetWindowPos(existing, None, x, by, w, h, SWP_NOZORDER).ok();
                let _ = SetWindowTextW(existing, &hstring(label));
                SendMessageW(
                    existing,
                    WM_SETFONT,
                    WPARAM(hf_bridge.0 as usize),
                    LPARAM(1),
                );
                let _ = ShowWindow(existing, SW_SHOW);
            }
        };

    if local_folder_valid {
        place_btn(
            hwnd,
            hi,
            IDC_OPEN_LOCAL_FOLDER,
            "Open",
            M + layout.open_btn_x,
            layout.open_btn_y,
            layout.open_btn_w,
            BRIDGE_BTN_H,
        );
        place_btn(
            hwnd,
            hi,
            IDC_BROWSE_LOCAL,
            "Choose",
            M + layout.browse_btn_x,
            layout.btn_y,
            layout.browse_btn_w,
            BRIDGE_BTN_H,
        );
        if is_paired(cfg) {
            place_btn(
                hwnd,
                hi,
                IDC_REFRESH_REMOTE,
                "Restore",
                M + layout.refresh_btn_x,
                layout.btn_y,
                layout.refresh_btn_w,
                BRIDGE_BTN_H,
            );
            place_btn(
                hwnd,
                hi,
                IDC_PAIR_DEVICE,
                pair_label,
                M + layout.refresh_btn_x + layout.refresh_btn_w + PAD,
                layout.btn_y,
                BRIDGE_RECONNECT_BTN_W,
                BRIDGE_BTN_H,
            );
        } else {
            place_btn(
                hwnd,
                hi,
                IDC_PAIR_DEVICE,
                pair_label,
                M + layout.pair_btn_x,
                layout.btn_y,
                layout.pair_btn_w,
                BRIDGE_BTN_H,
            );
            let _ = ShowWindow(GetDlgItem(hwnd, IDC_REFRESH_REMOTE as i32), SW_HIDE);
        }
    } else {
        let choose_w = BRIDGE_PAIR_BTN_W;
        place_btn(
            hwnd,
            hi,
            IDC_BROWSE_LOCAL,
            "Choose folder",
            M + (inner_w - choose_w) / 2,
            layout.btn_y,
            choose_w,
            BRIDGE_BTN_H,
        );
        let _ = ShowWindow(GetDlgItem(hwnd, IDC_OPEN_LOCAL_FOLDER as i32), SW_HIDE);
        let _ = ShowWindow(GetDlgItem(hwnd, IDC_PAIR_DEVICE as i32), SW_HIDE);
        let _ = ShowWindow(GetDlgItem(hwnd, IDC_REFRESH_REMOTE as i32), SW_HIDE);
    }

    y += card_h;
    if show_band {
        y += SECT;
        (*st).bridge_progress_rect = RECT {
            left: M,
            top: y,
            right: M + inner_w,
            bottom: y + SYNC_BAND_H,
        };
        y += SYNC_BAND_H;
    } else {
        (*st).bridge_progress_rect = RECT::default();
    }

    let prog = GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32);
    if prog.0.is_null() {
        let prog_hwnd = mkprogress(hwnd, hi, IDC_SYNC_PROGRESS, 0, 0, 1, 1);
        SendMessageW(
            prog_hwnd,
            PBM_SETBARCOLOR,
            WPARAM(0),
            LPARAM(C_BLUE as isize),
        );
        SendMessageW(
            prog_hwnd,
            PBM_SETBKCOLOR,
            WPARAM(0),
            LPARAM(C_PROGRESS_TRACK as isize),
        );
        ShowWindow(prog_hwnd, SW_HIDE);
    } else {
        ShowWindow(prog, SW_HIDE);
        SetWindowPos(prog, None, 0, 0, 1, 1, SWP_NOZORDER).ok();
    }

    y += GAP;
    y
}

fn server_tooltip_text(cfg: &Config) -> String {
    let url = if cfg.s3_endpoint.trim().is_empty() {
        "not set"
    } else {
        cfg.s3_endpoint.trim()
    };
    let folder = if cfg.remote_folder.trim().is_empty() {
        "waiting for Laravel approval"
    } else {
        cfg.remote_folder.trim()
    };
    let mut lines = vec![format!("Server: {url}"), format!("Destination: {folder}")];
    if matches!(
        crate::config::transport_kind(cfg),
        Some(crate::config::TransportKind::S3)
    ) && !cfg.s3_prefix.trim().is_empty()
    {
        lines.push(format!("Prefix: {}", cfg.s3_prefix.trim()));
    }
    if let Some(approved_at) = cfg.server_approved_at.as_deref().and_then(non_empty_str) {
        lines.push(format!("Approved: {approved_at}"));
    }
    if let Some(profile_id) = cfg.credential_profile_id {
        lines.push(format!("Credential profile: {profile_id}"));
    }
    if let Some(version) = cfg.credential_version {
        lines.push(format!("Credential version: {version}"));
    }
    lines.join("\n")
}

fn server_display_text(cfg: &Config) -> String {
    if cfg.s3_endpoint.trim().is_empty() {
        "Server not configured".to_string()
    } else {
        cfg.s3_endpoint
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
    if !cfg.remote_folder.trim().is_empty() {
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
        IDC_REFRESH_REMOTE,
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
