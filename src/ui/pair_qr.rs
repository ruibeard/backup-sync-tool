unsafe fn show_pair_qr_window(parent: HWND) {
    let st = stmut(parent);
    if !st.pair_qr_hwnd.0.is_null() && IsWindow(st.pair_qr_hwnd).as_bool() {
        DestroyWindow(st.pair_qr_hwnd).ok();
        st.pair_qr_hwnd = HWND(std::ptr::null_mut());
    }

    let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
    let hfont = mkfont_px("Segoe UI", FONT_BODY_PX, FW_NORMAL.0 as i32);
    let hfont_b = mkfont_px("Segoe UI", FONT_EMPHASIS_PX, FW_SEMIBOLD.0 as i32);
    let hfont_code = mkfont_px("Segoe UI", 20, FW_SEMIBOLD.0 as i32);
    let api_base = st.config.pair_api_base.clone();
    let state = Box::new(PairQrState {
        parent,
        api_base,
        code: String::new(),
        approve_url: String::new(),
        ready: false,
        hfont,
        hfont_b,
        hfont_code,
    });

    let hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        PAIR_QR_CLASS_NAME,
        w!("Pair Backup Sync Tool"),
        WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        100,
        100,
        parent,
        None,
        hinstance,
        Some(Box::into_raw(state) as *const c_void),
    );

    if hwnd.0.is_null() {
        return;
    }

    let mut rc = RECT {
        left: 0,
        top: 0,
        right: PAIR_QR_CLIENT_W,
        bottom: PAIR_QR_CLIENT_H,
    };
    AdjustWindowRectEx(
        &mut rc,
        WS_CAPTION | WS_SYSMENU | WS_POPUP,
        false,
        WS_EX_DLGMODALFRAME,
    )
    .ok();
    SetWindowPos(
        hwnd,
        None,
        0,
        0,
        rc.right - rc.left,
        rc.bottom - rc.top,
        SWP_NOMOVE | SWP_NOZORDER,
    )
    .ok();
    center_child_window(parent, hwnd, PAIR_QR_CLIENT_W, PAIR_QR_CLIENT_H);

    stmut(parent).pair_qr_hwnd = hwnd;
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
}

unsafe fn update_pair_qr_window(parent: HWND, code: &str, approve_url: &str) {
    let hwnd = stmut(parent).pair_qr_hwnd;
    if hwnd.0.is_null() || !IsWindow(hwnd).as_bool() {
        return;
    }

    let plane = {
        let st = pair_qr_state(hwnd);
        st.code = code.to_string();
        st.approve_url = approve_url.to_string();
        st.ready = true;
        st.api_base.clone()
    };

    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PAIR_QR_TITLE as i32),
        &hstring("Scan to pair with the server"),
    );
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PAIR_QR_SERVER as i32),
        &hstring(&plane),
    );
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PAIR_QR_STATUS as i32),
        &hstring("Waiting for admin approval..."),
    );
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PAIR_QR_CODE as i32),
        &hstring(&format!("Code: {code}")),
    );
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PAIR_QR_LINK as i32),
        &hstring(approve_url),
    );
    InvalidateRect(hwnd, None, TRUE);
    UpdateWindow(hwnd);
}

unsafe extern "system" fn pair_qr_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(1)
        }
        WM_CREATE => {
            pair_qr_on_create(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            pair_qr_paint(hwnd, hdc);
            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wp.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lp.0 as *mut _)) as u16;
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(
                hdc,
                COLORREF(if id == IDC_PAIR_QR_LINK {
                    C_BLUE
                } else if id == IDC_PAIR_QR_SERVER {
                    0x00777777
                } else {
                    C_LABEL
                }),
            );
            LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
        }
        WM_DRAWITEM => on_draw_item(lp),
        WM_COMMAND => {
            let id = (wp.0 & 0xFFFF) as u16;
            match id {
                IDC_PAIR_QR_LINK => {
                    let st = pair_qr_state(hwnd);
                    if !st.ready {
                        return LRESULT(0);
                    }
                    let _ = windows::Win32::UI::Shell::ShellExecuteW(
                        Some(hwnd),
                        w!("open"),
                        &hstring(&st.approve_url),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
                IDC_PAIR_QR_CANCEL => {
                    let st = pair_qr_state(hwnd);
                    cancel_pairing_from_popup(st.parent);
                    DestroyWindow(hwnd).ok();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            DestroyWindow(hwnd).ok();
            LRESULT(0)
        }
        WM_DESTROY => {
            pair_qr_on_destroy(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn pair_qr_on_create(hwnd: HWND) {
    let st = pair_qr_state(hwnd);
    let hi: HINSTANCE = GetModuleHandleW(None).unwrap().into();
    let margin = 18;
    mkstatic_align(
        hwnd,
        hi,
        IDC_PAIR_QR_TITLE,
        "Preparing pairing request...",
        margin,
        14,
        PAIR_QR_CLIENT_W - margin * 2,
        22,
        st.hfont_b,
        SS_CENTER,
    );
    mkstatic_align(
        hwnd,
        hi,
        IDC_PAIR_QR_SERVER,
        &st.api_base,
        margin,
        38,
        PAIR_QR_CLIENT_W - margin * 2,
        20,
        st.hfont,
        SS_CENTER,
    );
    mkstatic_align(
        hwnd,
        hi,
        IDC_PAIR_QR_STATUS,
        "Contacting server...",
        margin,
        352,
        PAIR_QR_CLIENT_W - margin * 2,
        22,
        st.hfont_b,
        SS_CENTER,
    );
    mkstatic_align(
        hwnd,
        hi,
        IDC_PAIR_QR_CODE,
        "Code: pending",
        margin,
        380,
        PAIR_QR_CLIENT_W - margin * 2,
        28,
        st.hfont_code,
        SS_CENTER,
    );
    mkstatic_align(
        hwnd,
        hi,
        0,
        "This code expires in 5 minutes",
        margin,
        412,
        PAIR_QR_CLIENT_W - margin * 2,
        20,
        st.hfont,
        SS_CENTER,
    );
    mkstatic_align(
        hwnd,
        hi,
        IDC_PAIR_QR_LINK,
        "",
        margin,
        438,
        PAIR_QR_CLIENT_W - margin * 2,
        20,
        st.hfont,
        SS_CENTER | SS_NOTIFY,
    );
    mkbtn_grey(
        hwnd,
        hi,
        IDC_PAIR_QR_CANCEL,
        "Cancel",
        (PAIR_QR_CLIENT_W - ACTION_BTN_W) / 2,
        472,
        ACTION_BTN_W,
        ACTION_BTN_H,
        st.hfont,
    );
}

unsafe fn pair_qr_paint(hwnd: HWND, hdc: HDC) {
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let br_bg = CreateSolidBrush(COLORREF(C_WIN_BG));
    FillRect(hdc, &cr, br_bg);
    DeleteObject(br_bg);

    let st = pair_qr_state(hwnd);
    if !st.ready {
        let br_white = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let outer = RECT {
            left: 52,
            top: 64,
            right: PAIR_QR_CLIENT_W - 52,
            bottom: 336,
        };
        FillRect(hdc, &outer, br_white);
        DeleteObject(br_white);
        let mut text_rc = RECT {
            left: 78,
            top: 178,
            right: PAIR_QR_CLIENT_W - 78,
            bottom: 224,
        };
        SetTextColor(hdc, COLORREF(C_LABEL));
        SetBkMode(hdc, TRANSPARENT);
        SelectObject(hdc, st.hfont_b);
        let mut text: Vec<u16> = "Generating QR code...".encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut text,
            &mut text_rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        return;
    }

    let qr = match QrCode::encode_text(&st.approve_url, QrCodeEcc::Medium) {
        Ok(qr) => qr,
        Err(_) => return,
    };
    let modules = qr.size();
    let qr_px = 260;
    let scale = (qr_px / modules).max(1);
    let drawn = modules * scale;
    let left = (PAIR_QR_CLIENT_W - drawn) / 2;
    let top = 72;

    let quiet = 8;
    let br_white = CreateSolidBrush(COLORREF(0x00FFFFFF));
    let outer = RECT {
        left: left - quiet,
        top: top - quiet,
        right: left + drawn + quiet,
        bottom: top + drawn + quiet,
    };
    FillRect(hdc, &outer, br_white);
    DeleteObject(br_white);

    let br_black = CreateSolidBrush(COLORREF(0x00000000));
    for y in 0..modules {
        for x in 0..modules {
            if qr.get_module(x, y) {
                let rc = RECT {
                    left: left + x * scale,
                    top: top + y * scale,
                    right: left + (x + 1) * scale,
                    bottom: top + (y + 1) * scale,
                };
                FillRect(hdc, &rc, br_black);
            }
        }
    }
    DeleteObject(br_black);
}

unsafe fn pair_qr_on_destroy(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PairQrState;
    if ptr.is_null() {
        return;
    }
    let st = Box::from_raw(ptr);
    let parent = st.parent;
    if !parent.0.is_null() && IsWindow(parent).as_bool() {
        cancel_pairing_from_popup(parent);
        let parent_state = state_ptr(parent);
        if !parent_state.is_null() && (*parent_state).pair_qr_hwnd == hwnd {
            (*parent_state).pair_qr_hwnd = HWND(std::ptr::null_mut());
        }
    }
    DeleteObject(st.hfont);
    DeleteObject(st.hfont_b);
    DeleteObject(st.hfont_code);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
}

unsafe fn pair_qr_state(hwnd: HWND) -> &'static mut PairQrState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PairQrState)
}

unsafe fn center_child_window(parent: HWND, child: HWND, child_w: i32, child_h: i32) {
    let mut pr = RECT::default();
    GetWindowRect(parent, &mut pr).ok();
    let x = pr.left + ((pr.right - pr.left) - child_w) / 2;
    let y = pr.top + ((pr.bottom - pr.top) - child_h) / 2;
    SetWindowPos(
        child,
        None,
        x.max(0),
        y.max(0),
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER,
    )
    .ok();
}
