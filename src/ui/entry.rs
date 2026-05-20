// ── Entry point ───────────────────────────────────────────────────────────────
pub fn run(hinstance: HINSTANCE, start_minimized: bool) {
    unsafe {
        let icex = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icex);

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: CLASS_NAME,
            hIcon: LoadIconW(hinstance, w!("APP_ICON_IDLE"))
                .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let pair_qr_wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(pair_qr_wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            lpszClassName: PAIR_QR_CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&pair_qr_wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            w!("Backup Sync Tool"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_W,
            100,
            None,
            None,
            hinstance,
            None,
        );
        ShowWindow(hwnd, if start_minimized { SW_HIDE } else { SW_SHOW });
        UpdateWindow(hwnd);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 || ret.0 == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ── Window procedure ──────────────────────────────────────────────────────────
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            on_create(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            paint_bg(hwnd, hdc);
            EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        // Static / label controls
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut _);
            let hctl = HWND(lparam.0 as *mut _);
            let id = GetDlgCtrlID(hctl) as u16;
            SetBkMode(hdc, TRANSPARENT);
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            if id == IDC_REMOTE_FOLDER {
                SetTextColor(hdc, COLORREF(C_LABEL));
                return LRESULT((*st).br_path_box.0 as isize);
            }
            if id == IDC_SYNC_STATUS {
                SetTextColor(
                    hdc,
                    COLORREF(if (*st).sync_footer_busy {
                        C_LABEL
                    } else {
                        C_STATUS_MUTED
                    }),
                );
                let br = if (*st).sync_footer_busy {
                    (*st).br_footer_busy
                } else {
                    (*st).br_footer_idle
                };
                return LRESULT(br.0 as isize);
            }
            if id == IDC_SYNC_ETA {
                SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
                let br = if (*st).sync_footer_busy {
                    (*st).br_footer_busy
                } else {
                    (*st).br_footer_idle
                };
                return LRESULT(br.0 as isize);
            }
            let text_clr = match id {
                IDC_DEST_CREATED => C_GREEN,
                IDC_REPO => C_BLUE,
                IDC_AUTHOR => C_LABEL,
                IDC_SERVER_HDR | IDC_ACTIVITY_HDR => 0x00888888,
                IDC_SERVER_URL_LABEL => 0x00777777,
                IDC_ORIGIN_LABEL | IDC_DEST_LABEL => 0x00555555,
                _ => C_LABEL,
            };
            SetTextColor(hdc, COLORREF(text_clr));
            LRESULT((*st).br_win.0 as isize)
        }

        WM_CTLCOLORLISTBOX => {
            let hdc = HDC(wparam.0 as *mut _);
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            SetBkColor(hdc, COLORREF(C_INPUT_BG));
            LRESULT((*st).br_input.0 as isize)
        }

        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut _);
            SetBkColor(hdc, COLORREF(C_INPUT_BG));
            SetTextColor(hdc, COLORREF(C_LABEL));
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            LRESULT((*st).br_input.0 as isize)
        }

        WM_CTLCOLORBTN => {
            let hdc = HDC(wparam.0 as *mut _);
            SetBkMode(hdc, TRANSPARENT);
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(NULL_BRUSH).0 as isize);
            }
            LRESULT((*st).br_win.0 as isize)
        }

        WM_COMMAND => on_command(hwnd, wparam),
        WM_MEASUREITEM => on_measure_item(hwnd, lparam),
        WM_DRAWITEM => on_draw_item(lparam),

        WM_GETMINMAXINFO => {
            let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
            let st = state_ptr(hwnd);
            if !st.is_null() && (*st).min_client_h > 0 {
                // Calculate frame sizes
                let mut wr_test = RECT {
                    left: 0,
                    top: 0,
                    right: WIN_W,
                    bottom: (*st).min_client_h,
                };
                let _ = AdjustWindowRectEx(
                    &mut wr_test,
                    WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME,
                    false,
                    WINDOW_EX_STYLE::default(),
                );
                let frame_w = wr_test.right - wr_test.left;
                let frame_h = wr_test.bottom - wr_test.top;
                // Lock width, set min height
                mmi.ptMinTrackSize = POINT {
                    x: frame_w,
                    y: frame_h,
                };
                mmi.ptMaxTrackSize.x = frame_w; // lock horizontal
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let st = state_ptr(hwnd);
            if !st.is_null() && (*st).min_client_h > 0 {
                layout_main(hwnd);
            }
            LRESULT(0)
        }

        tray::WM_TRAY => on_tray(hwnd, lparam),
        WM_APP_LOG => on_app_log(hwnd, lparam),
        WM_APP_CONNECTED => on_app_connected(hwnd, wparam),
        WM_APP_UPDATE => on_app_update(hwnd, wparam, lparam),
        WM_APP_REMOTE_FOLDER => on_app_remote_folder(hwnd, lparam),
        WM_APP_SYNC_ACTIVITY => on_app_sync_activity(hwnd, wparam, lparam),
        WM_APP_PAIR_STARTED => on_app_pair_started(hwnd, lparam),
        WM_APP_PAIR_RESULT => on_app_pair_result(hwnd, wparam, lparam),
        WM_APP_AUTH_FAILED => on_app_auth_failed(hwnd),
        WM_TIMER => on_timer(hwnd, wparam),

        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState;
            if !st.is_null() {
                tray::remove_tray_icon(hwnd);
                DeleteObject((*st).br_win);
                DeleteObject((*st).br_status_strip);
                DeleteObject((*st).br_path_box);
                DeleteObject((*st).br_footer_idle);
                DeleteObject((*st).br_footer_busy);
                DeleteObject((*st).br_sect);
                DeleteObject((*st).br_input);
                DeleteObject((*st).hfont);
                DeleteObject((*st).hfont_hdr);
                DeleteObject((*st).hfont_b);
                DeleteObject((*st).hfont_small);
                DeleteObject((*st).hfont_activity);
                DeleteObject((*st).hfont_btn);
                DeleteObject((*st).hfont_link);
                drop(Box::from_raw(st));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
