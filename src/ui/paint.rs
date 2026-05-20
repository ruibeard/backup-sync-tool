// ── Background paint ──────────────────────────────────────────────────────────
unsafe fn fill_rect_color(hdc: HDC, rc: &RECT, color: u32) {
    let br = CreateSolidBrush(COLORREF(color));
    FillRect(hdc, rc, br);
    DeleteObject(br);
}

unsafe fn frame_rect_color(hdc: HDC, rc: &RECT, color: u32) {
    let hp = CreatePen(PS_SOLID, 1, COLORREF(color));
    let op = SelectObject(hdc, hp);
    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(hp);
}

unsafe fn draw_status_strip_text(
    hdc: HDC,
    sr: &RECT,
    dot_r: &RECT,
    primary: &str,
    secondary: &str,
    hf: HFONT,
    hf_b: HFONT,
    hf_small: HFONT,
) {
    let mut tr = *sr;
    tr.left = dot_r.right + 8;
    tr.top += 6;
    tr.bottom -= 6;

    SetBkMode(hdc, TRANSPARENT);
    let of = SelectObject(hdc, hf_b);
    SetTextColor(hdc, COLORREF(C_LABEL));

    let draw_segment = |hdc: HDC, tr: &mut RECT, text: &str, font: HFONT, color: u32| -> i32 {
        let prev = SelectObject(hdc, font);
        SetTextColor(hdc, COLORREF(color));
        let mut w: Vec<u16> = text.encode_utf16().collect();
        let mut seg_rc = *tr;
        DrawTextW(
            hdc,
            &mut w,
            &mut seg_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_CALCRECT,
        );
        let width = seg_rc.right - seg_rc.left;
        seg_rc.right = tr.left + width;
        DrawTextW(
            hdc,
            &mut w,
            &mut seg_rc,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(hdc, prev);
        width
    };

    if !secondary.is_empty() {
        let head_w_px = draw_segment(hdc, &mut tr, primary.trim(), hf_b, C_LABEL);
        tr.left += head_w_px;
        let _ = draw_segment(
            hdc,
            &mut tr,
            &format!(" \u{00B7} {secondary}"),
            hf_small,
            C_STATUS_MUTED,
        );
        SelectObject(hdc, of);
        return;
    }

    let (head, tail) = if let Some(idx) = primary.find('\u{00B7}') {
        let (h, t) = primary.split_at(idx);
        (h.trim(), Some(t.trim()))
    } else {
        (primary.trim(), None)
    };

    let head_w_px = draw_segment(hdc, &mut tr, head, hf_b, C_LABEL);
    if let Some(tail) = tail {
        tr.left += head_w_px;
        let _ = draw_segment(
            hdc,
            &mut tr,
            &format!(" \u{00B7} {tail}"),
            hf,
            C_LABEL,
        );
    }
    SelectObject(hdc, of);
}

unsafe fn paint_bg(hwnd: HWND, hdc: HDC) {
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();

    fill_rect_color(hdc, &cr, C_WIN_BG);

    let st = state_ptr(hwnd);
    if st.is_null() {
        return;
    }

    let accent_color = (*st).status_dot_color;
    let status_text = (*st).status_strip_display.clone();
    let status_secondary = (*st).status_strip_secondary.clone();
    let hf = (*st).hfont;
    let hf_b = (*st).hfont_b;
    let hf_small = (*st).hfont_small;

    let sr = (*st).status_strip_rect;
    if sr.right > sr.left && sr.bottom > sr.top {
        fill_rect_color(hdc, &sr, C_STATUS_BG);
        let accent = RECT {
            left: sr.left,
            top: sr.top,
            right: sr.left + STATUS_ACCENT_W,
            bottom: sr.bottom,
        };
        fill_rect_color(hdc, &accent, accent_color);

        let dot_r = (*st).server_status_rect;
        if dot_r.right > dot_r.left {
            let br_dot = CreateSolidBrush(COLORREF(accent_color));
            let op_br = SelectObject(hdc, br_dot);
            Ellipse(hdc, dot_r.left, dot_r.top, dot_r.right, dot_r.bottom);
            SelectObject(hdc, op_br);
            DeleteObject(br_dot);
        }

        if !status_text.is_empty() {
            draw_status_strip_text(
                hdc,
                &sr,
                &dot_r,
                &status_text,
                &status_secondary,
                hf,
                hf_b,
                hf_small,
            );
        }
    }

    let dr = (*st).dest_path_rect;
    if dr.right > dr.left && dr.bottom > dr.top {
        fill_rect_color(hdc, &dr, C_DEST_PATH_BG);
        let hp = CreatePen(PS_SOLID, 1, COLORREF(C_DEST_PATH_BORDER));
        let op = SelectObject(hdc, hp);
        let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        RoundRect(hdc, dr.left, dr.top, dr.right, dr.bottom, 4, 4);
        SelectObject(hdc, op);
        SelectObject(hdc, ob);
        DeleteObject(hp);
    }

    let ar = (*st).activity_list_rect;
    if ar.right > ar.left && ar.bottom > ar.top {
        fill_rect_color(hdc, &ar, C_INPUT_BG);
        frame_rect_color(hdc, &ar, C_PANEL_BORDER);
    }

    let fr = (*st).sync_footer_rect;
    if fr.right > fr.left && fr.bottom > fr.top {
        if (*st).sync_footer_busy {
            fill_rect_color(hdc, &fr, C_STATUS_BG);
            frame_rect_color(hdc, &fr, C_FOOTER_BUSY_BORDER);
        } else {
            fill_rect_color(hdc, &fr, C_FOOTER_IDLE_BG);
            frame_rect_color(hdc, &fr, C_FOOTER_IDLE_BORDER);
        }
    }

    for &dy in &(*st).dividers {
        if dy <= 0 {
            continue;
        }
        let hp = CreatePen(PS_SOLID, 1, COLORREF(C_DIVIDER));
        let op = SelectObject(hdc, hp);
        let _ = MoveToEx(hdc, M, dy, None);
        let _ = LineTo(hdc, WIN_W - M, dy);
        SelectObject(hdc, op);
        DeleteObject(hp);
    }
}

// ── Edit subclass: flat 1px border ───────────────────────────────────────────
unsafe extern "system" fn edit_sub(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _ref: usize,
) -> LRESULT {
    let id = GetDlgCtrlID(hwnd) as u16;

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

            ReleaseDC(hwnd, hdc);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}
