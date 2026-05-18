// ── Background paint ──────────────────────────────────────────────────────────
// Paints window bg, divider lines, and inline status dot + text.
unsafe fn paint_bg(hwnd: HWND, hdc: HDC) {
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();

    // Window fill
    let br = CreateSolidBrush(COLORREF(C_WIN_BG));
    FillRect(hdc, &cr, br);
    DeleteObject(br);

    let st = state_ptr(hwnd);
    if st.is_null() {
        return;
    }

    if (*st).sync_icon.0 != 0 {
        let r = (*st).sync_icon_rect;
        let _ = DrawIconEx(
            hdc,
            r.left,
            r.top,
            (*st).sync_icon,
            r.right - r.left,
            r.bottom - r.top,
            0,
            HBRUSH(0),
            DI_NORMAL,
        );
    }

    // Subtle divider lines between sections
    for &dy in &(*st).dividers {
        let hp = CreatePen(PS_SOLID, 1, COLORREF(C_DIVIDER));
        let op = SelectObject(hdc, hp);
        MoveToEx(hdc, M, dy, None);
        LineTo(hdc, WIN_W - M, dy);
        SelectObject(hdc, op);
        DeleteObject(hp);
    }
}

// ── Edit subclass: flat 1px border + eye icon for password field ──────────────
//
// For IDC_PASSWORD:
//   - WM_NCPAINT draws the border AND an eye glyph in the right padding.
//   - WM_NCLBUTTONDOWN within the eye zone toggles password visibility.
//   - WM_NCHITTEST returns HTCAPTION over the eye zone so WM_NCLBUTTONDOWN fires.
unsafe extern "system" fn edit_sub(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _ref: usize,
) -> LRESULT {
    let id = GetDlgCtrlID(hwnd) as u16;
    let is_pw = id == IDC_PASSWORD;

    match msg {
        WM_SETFOCUS | WM_KILLFOCUS => {
            let st = state_ptr(GetParent(hwnd));
            if !st.is_null() {
                (*st).focused_edit = if msg == WM_SETFOCUS { id } else { 0 };
            }
            let r = DefSubclassProc(hwnd, msg, wp, lp);
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            )
            .ok();
            r
        }
        WM_NCPAINT => {
            let st = state_ptr(GetParent(hwnd));
            let focused = !st.is_null() && (*st).focused_edit == id;
            let hdc = GetWindowDC(hwnd);
            let mut wr = RECT::default();
            GetWindowRect(hwnd, &mut wr).ok();
            let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
            let border_clr = if focused {
                C_INPUT_FOCUS
            } else {
                C_INPUT_BORDER
            };

            let hpen = CreatePen(PS_SOLID, 1, COLORREF(border_clr));
            let op = SelectObject(hdc, hpen);
            let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            Rectangle(hdc, 0, 0, w, h);
            SelectObject(hdc, op);
            SelectObject(hdc, ob);
            DeleteObject(hpen);

            // Eye glyph for password field
            if is_pw && !st.is_null() {
                draw_eye(hdc, w, h, (*st).pw_visible);
            }

            ReleaseDC(hwnd, hdc);
            LRESULT(0)
        }
        WM_NCHITTEST if is_pw => {
            // Check if cursor is in the eye zone (right edge of non-client area)
            let pt = POINT {
                x: (lp.0 & 0xFFFF) as i16 as i32,
                y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let mut wr = RECT::default();
            GetWindowRect(hwnd, &mut wr).ok();
            let right = wr.right;
            let top = wr.top;
            let bottom = wr.bottom;
            if pt.x >= right - EYE_ZONE_W && pt.x < right && pt.y >= top && pt.y < bottom {
                return LRESULT(HTCAPTION as isize);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_NCLBUTTONDOWN if is_pw => {
            if wp.0 as u32 == HTCAPTION {
                // Eye icon clicked — toggle password visibility
                let parent = GetParent(hwnd);
                let st = stmut(parent);
                st.pw_visible = !st.pw_visible;
                let pw_char = if st.pw_visible { 0u32 } else { 0x2022 };
                SendMessageW(
                    hwnd,
                    EM_SETPASSWORDCHAR,
                    WPARAM(pw_char as usize),
                    LPARAM(0),
                );
                InvalidateRect(hwnd, None, TRUE);
                // Force NC repaint for the eye icon update
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                )
                .ok();
                return LRESULT(0);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

/// Draw an eye icon glyph in the non-client right area of an edit control.
/// `w`/`h` are the full window rect dimensions. Uses GDI arcs + ellipse.
unsafe fn draw_eye(hdc: HDC, w: i32, h: i32, open: bool) {
    let cx = w - EYE_ZONE_W / 2;
    let cy = h / 2;
    let r = 4i32; // pupil radius
    let lw = 10i32; // half-width of eyelid arc bounding box

    SetBkMode(hdc, TRANSPARENT);

    let hpen = CreatePen(PS_SOLID, 1, COLORREF(C_EYE));
    let op = SelectObject(hdc, hpen);
    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));

    if open {
        // Upper arc (eyelid top): Arc from left to right via top
        Arc(
            hdc,
            cx - lw,
            cy - r - 3,
            cx + lw,
            cy + r + 3,
            cx - lw,
            cy,
            cx + lw,
            cy,
        );
        // Lower arc (eyelid bottom)
        Arc(
            hdc,
            cx - lw,
            cy - r - 3,
            cx + lw,
            cy + r + 3,
            cx + lw,
            cy,
            cx - lw,
            cy,
        );
        // Pupil
        let pb = CreateSolidBrush(COLORREF(C_EYE));
        let opb = SelectObject(hdc, pb);
        Ellipse(hdc, cx - r + 1, cy - r + 1, cx + r, cy + r);
        SelectObject(hdc, opb);
        DeleteObject(pb);
    } else {
        // Closed eye — just a single horizontal arc (top lid only, flat)
        Arc(
            hdc,
            cx - lw,
            cy - r - 1,
            cx + lw,
            cy + r + 4,
            cx - lw,
            cy + 2,
            cx + lw,
            cy + 2,
        );
        // Three small eyelash lines below
        let hp2 = CreatePen(PS_SOLID, 1, COLORREF(C_EYE));
        let op2 = SelectObject(hdc, hp2);
        for i in [-4i32, 0, 4] {
            MoveToEx(hdc, cx + i, cy + 4, None);
            LineTo(hdc, cx + i, cy + 7);
        }
        SelectObject(hdc, op2);
        DeleteObject(hp2);
    }

    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(hpen);
}

