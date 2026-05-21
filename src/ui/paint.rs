// ── Background paint ──────────────────────────────────────────────────────────
const BRIDGE_PC_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/bridge-pc.png"));
const BRIDGE_SERVER_PNG: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/bridge-server.png"));

unsafe fn png_bytes_to_hbitmap(bytes: &[u8], target_px: i32) -> HBITMAP {
    let img = match image::load_from_memory(bytes) {
        Ok(img) => {
            let px = target_px.max(BRIDGE_ICO) as u32;
            if img.width() != px || img.height() != px {
                img.resize_exact(px, px, image::imageops::FilterType::Lanczos3)
                    .to_rgba8()
            } else {
                img.to_rgba8()
            }
        }
        Err(_) => return HBITMAP(std::ptr::null_mut()),
    };
    let w = img.width() as i32;
    let h = img.height() as i32;
    if w <= 0 || h <= 0 {
        return HBITMAP(std::ptr::null_mut());
    }

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let screen = GetDC(None);
    let hbmp = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .unwrap_or(HBITMAP(std::ptr::null_mut()));
    ReleaseDC(None, screen);
    if hbmp.0.is_null() || bits.is_null() {
        return HBITMAP(std::ptr::null_mut());
    }

    let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x as u32, y as u32);
            let i = ((y * w + x) * 4) as usize;
            dst[i] = px[2];
            dst[i + 1] = px[1];
            dst[i + 2] = px[0];
            dst[i + 3] = px[3];
        }
    }
    hbmp
}

unsafe fn load_bridge_icons(hwnd: HWND) -> (HBITMAP, HBITMAP) {
    let hdc = GetDC(Some(hwnd));
    let dpi = if hdc.0.is_null() {
        96
    } else {
        GetDeviceCaps(hdc, LOGPIXELSY)
    };
    if !hdc.0.is_null() {
        ReleaseDC(Some(hwnd), hdc);
    }
    let target_px = (BRIDGE_ICO * dpi + 48) / 96;
    (
        png_bytes_to_hbitmap(BRIDGE_PC_PNG, target_px),
        png_bytes_to_hbitmap(BRIDGE_SERVER_PNG, target_px),
    )
}

unsafe fn blit_hbitmap_alpha(hdc_dest: HDC, dest: &RECT, hbmp: HBITMAP) {
    if hbmp.0.is_null() {
        return;
    }
    let mem_dc = CreateCompatibleDC(Some(hdc_dest));
    let old = SelectObject(mem_dc, hbmp);
    let mut bm = BITMAP::default();
    let _ = GetObjectW(
        hbmp.into(),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut _),
    );
    let dest_w = dest.right - dest.left;
    let dest_h = dest.bottom - dest.top;
    let old_mode = SetStretchBltMode(hdc_dest, HALFTONE);
    let _ = SetBrushOrgEx(hdc_dest, dest.left % 8, dest.top % 8, None);
    let bf = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = GdiAlphaBlend(
        hdc_dest,
        dest.left,
        dest.top,
        dest_w,
        dest_h,
        mem_dc,
        0,
        0,
        bm.bmWidth,
        bm.bmHeight,
        bf,
    );
    SetStretchBltMode(hdc_dest, STRETCH_BLT_MODE(old_mode));
    SelectObject(mem_dc, old);
    DeleteDC(mem_dc);
}

unsafe fn draw_bridge_icon_png(hdc: HDC, rc: &RECT, hbmp: HBITMAP) {
    blit_hbitmap_alpha(hdc, rc, hbmp);
}

unsafe fn draw_bridge_node_name(
    hdc: HDC,
    rc: &RECT,
    name: &str,
    hf: HFONT,
    connection: Option<bool>,
) {
    let of = SelectObject(hdc, hf);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(C_LABEL));

    let Some(connected) = connection else {
        draw_text_w(
            hdc,
            rc,
            name,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        SelectObject(hdc, of);
        return;
    };

    let sym = if connected { "\u{2713}" } else { "\u{2717}" };
    let sym_color = if connected { C_GREEN } else { C_RED };
    let gap = 5;

    let mut name_w: Vec<u16> = name.encode_utf16().collect();
    let mut sym_w: Vec<u16> = sym.encode_utf16().collect();
    let mut name_rc = RECT::default();
    let mut sym_rc = RECT::default();
    DrawTextW(
        hdc,
        &mut name_w,
        &mut name_rc,
        DT_LEFT | DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX,
    );
    DrawTextW(
        hdc,
        &mut sym_w,
        &mut sym_rc,
        DT_LEFT | DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX,
    );
    let name_w_px = name_rc.right - name_rc.left;
    let sym_w_px = sym_rc.right - sym_rc.left;
    let total_w = name_w_px + gap + sym_w_px;
    let row_w = rc.right - rc.left;
    let start_x = rc.left + (row_w - total_w) / 2;

    let name_draw = RECT {
        left: start_x,
        top: rc.top,
        right: start_x + name_w_px,
        bottom: rc.bottom,
    };
    draw_text_w(
        hdc,
        &name_draw,
        name,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );

    SetTextColor(hdc, COLORREF(sym_color));
    let sym_draw = RECT {
        left: start_x + name_w_px + gap,
        top: rc.top,
        right: start_x + total_w,
        bottom: rc.bottom,
    };
    draw_text_w(
        hdc,
        &sym_draw,
        sym,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    SelectObject(hdc, of);
}

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

unsafe fn round_rect_color(hdc: HDC, rc: &RECT, color: u32, border: u32, radius: i32) {
    let br = CreateSolidBrush(COLORREF(color));
    let hp = CreatePen(PS_SOLID, 1, COLORREF(border));
    let ob = SelectObject(hdc, br);
    let op = SelectObject(hdc, hp);
    RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(hp);
    DeleteObject(br);
}

unsafe fn draw_text_w(
    hdc: HDC,
    rc: &RECT,
    text: &str,
    flags: DRAW_TEXT_FORMAT,
) {
    let mut w: Vec<u16> = text.encode_utf16().collect();
    let mut tr = *rc;
    DrawTextW(hdc, &mut w, &mut tr, flags | DT_NOPREFIX);
}

unsafe fn draw_status_pill_row(
    hdc: HDC,
    sr: &RECT,
    primary: &str,
    subtitle: &str,
    dot_color: u32,
    syncing: bool,
    hf_pill: HFONT,
    hf_small: HFONT,
) {
    SetBkMode(hdc, TRANSPARENT);

    let (pill_bg, pill_fg) = if syncing {
        (C_PILL_SYNC_BG, C_PILL_SYNC_TXT)
    } else if dot_color == C_GREEN {
        (C_PILL_GREEN_BG, C_GREEN)
    } else if dot_color == C_RED {
        (0x00E6E6FF_u32, C_RED)
    } else {
        (0x00E6F0FF_u32, C_AMBER)
    };

    let of_pill = SelectObject(hdc, hf_pill);
    let mut primary_w: Vec<u16> = primary.encode_utf16().collect();
    let mut pill_text_rc = RECT::default();
    DrawTextW(
        hdc,
        &mut primary_w,
        &mut pill_text_rc,
        DT_LEFT | DT_SINGLELINE | DT_CALCRECT,
    );
    let dot_size = 10i32;
    let pill_w = dot_size + 6 + (pill_text_rc.right - pill_text_rc.left) + 16;
    let pill_h = 24i32;
    let pill = RECT {
        left: sr.left,
        top: sr.top + (sr.bottom - sr.top - pill_h) / 2,
        right: sr.left + pill_w,
        bottom: sr.top + (sr.bottom - sr.top + pill_h) / 2,
    };
    round_rect_color(hdc, &pill, pill_bg, pill_bg, pill_h / 2);

    let dot_y = pill.top + (pill_h - dot_size) / 2;
    let dot = RECT {
        left: pill.left + 8,
        top: dot_y,
        right: pill.left + 8 + dot_size,
        bottom: dot_y + dot_size,
    };
    let br_dot = CreateSolidBrush(COLORREF(dot_color));
    let op_br = SelectObject(hdc, br_dot);
    Ellipse(hdc, dot.left, dot.top, dot.right, dot.bottom);
    SelectObject(hdc, op_br);
    DeleteObject(br_dot);

    SetTextColor(hdc, COLORREF(pill_fg));
    let mut text_rc = RECT {
        left: dot.right + 6,
        top: pill.top,
        right: pill.right - 4,
        bottom: pill.bottom,
    };
    DrawTextW(
        hdc,
        &mut primary_w,
        &mut text_rc,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
    SelectObject(hdc, of_pill);

    if !subtitle.is_empty() {
        let of_s = SelectObject(hdc, hf_small);
        SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
        let mut sub_rc = RECT {
            left: sr.left,
            top: sr.top,
            right: sr.right,
            bottom: sr.bottom,
        };
        let mut sub_w: Vec<u16> = subtitle.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut sub_w,
            &mut sub_rc,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, of_s);
    }
}

unsafe fn draw_bridge_icon_badge(hdc: HDC, ico: &RECT, ok: bool, hf: HFONT) {
    let badge = 18;
    let cx = ico.right - 4;
    let cy = ico.bottom - 4;
    let rc = RECT {
        left: cx - badge / 2,
        top: cy - badge / 2,
        right: cx + badge / 2,
        bottom: cy + badge / 2,
    };
    let color = if ok { C_BRIDGE_CONN_OK } else { C_BRIDGE_CONN_FAIL };
    round_rect_color(hdc, &rc, color, color, badge / 2);
    let sym = if ok { "\u{2713}" } else { "\u{2717}" };
    let of = SelectObject(hdc, hf);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(0x00FFFFFF));
    draw_text_w(
        hdc,
        &rc,
        sym,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
    );
    SelectObject(hdc, of);
}

fn offset_rect_x(rc: RECT, dx: i32) -> RECT {
    RECT {
        left: rc.left + dx,
        top: rc.top,
        right: rc.right + dx,
        bottom: rc.bottom,
    }
}

unsafe fn draw_sync_bridge(hdc: HDC, br: &RECT, st: &WndState) {
    let inner_w = br.right - br.left;
    let layout = bridge_layout_at(br.top, inner_w);
    let hf_name = st.hfont_bridge_name;
    let hf_path = st.hfont_bridge_path;
    let left_tile = offset_rect_x(layout.left_tile, br.left);
    let right_tile = offset_rect_x(layout.right_tile, br.left);
    let left_ico = offset_rect_x(layout.left_ico, br.left);
    let right_ico = offset_rect_x(layout.right_ico, br.left);
    let right_name = offset_rect_x(layout.right_name, br.left);
    let left_path = offset_rect_x(layout.left_path, br.left);
    let right_conn = offset_rect_x(layout.right_conn, br.left);

    round_rect_color(
        hdc,
        &left_tile,
        C_BRIDGE_ICO_BG,
        C_BRIDGE_ICO_BORDER,
        8,
    );
    round_rect_color(
        hdc,
        &right_tile,
        C_BRIDGE_ICO_BG,
        C_BRIDGE_ICO_BORDER,
        8,
    );

    let of_name = SelectObject(hdc, hf_name);
    SetBkMode(hdc, TRANSPARENT);

    draw_bridge_icon_png(hdc, &left_ico, st.bridge_icon_pc);
    draw_bridge_icon_png(hdc, &right_ico, st.bridge_icon_cloud);
    draw_bridge_icon_badge(hdc, &right_ico, st.bridge_conn_ok, st.hfont_small);

    SetTextColor(hdc, COLORREF(C_LABEL));

    let of_s = SelectObject(hdc, hf_path);
    draw_text_w(
        hdc,
        &right_name,
        &bridge_server_name(st),
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SetTextColor(
        hdc,
        COLORREF(if st.bridge_conn_ok {
            C_BRIDGE_CONN_OK
        } else {
            C_BRIDGE_CONN_FAIL
        }),
    );
    draw_text_w(
        hdc,
        &right_conn,
        &st.bridge_conn_label,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    SetTextColor(hdc, COLORREF(C_BRIDGE_PATH_TXT));
    draw_text_w(
        hdc,
        &left_path,
        &bridge_pc_path(st),
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    let divider = RECT {
        left: br.left,
        top: layout.divider_y,
        right: br.right,
        bottom: layout.divider_y + 1,
    };
    fill_rect_color(hdc, &divider, C_DIVIDER);

    SelectObject(hdc, of_s);
    SelectObject(hdc, of_name);
}

unsafe fn draw_sync_band(hdc: HDC, rc: &RECT, st: &WndState) {
    if rc.bottom <= rc.top || rc.right <= rc.left {
        return;
    }

    let hf_detail = st.hfont_small;
    let syncing = bridge_syncing_progress(st);
    let checking = st.sync_status_state == crate::sync::ActivityState::Checking as usize;
    let all_synced = st.bridge_sync_head == "All synced";

    let (head, pct, bar_color, detail, eta) = if syncing {
        let done = st.sync_progress_done.min(st.sync_progress_total);
        let total = st.sync_progress_total;
        let pct = if total > 0 {
            (done * 100) / total
        } else {
            0
        };
        let detail = if total > 0 {
            format!("{done} of {total} files")
        } else {
            String::new()
        };
        let eta = st
            .sync_status_text
            .split("ETA ")
            .nth(1)
            .and_then(|s| s.split('\u{00B7}').next())
            .map(|eta| format!("ETA {eta}"))
            .unwrap_or_default();
        (
            "Syncing".to_string(),
            pct,
            C_BLUE,
            detail,
            eta,
        )
    } else if checking {
        ("Checking…".to_string(), 0, C_PROGRESS_TRACK, String::new(), String::new())
    } else if all_synced {
        ("All synced".to_string(), 100, C_GREEN, String::new(), String::new())
    } else {
        (
            st.bridge_sync_head.clone(),
            0,
            C_PROGRESS_TRACK,
            String::new(),
            String::new(),
        )
    };

    let head_color = if syncing {
        C_BRIDGE_SYNC_HEAD_ACTIVE
    } else if all_synced {
        C_BRIDGE_SYNC_HEAD_OK
    } else {
        C_BRIDGE_SYNC_HEAD_IDLE
    };

    let row1 = RECT {
        left: rc.left,
        top: rc.top,
        right: rc.right,
        bottom: rc.top + 20,
    };
    let of_h = SelectObject(hdc, st.hfont_b);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(head_color));
    draw_text_w(
        hdc,
        &row1,
        &head,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
    if pct > 0 || all_synced {
        let pct_text = format!("{pct}%");
        draw_text_w(
            hdc,
            &row1,
            &pct_text,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
    SelectObject(hdc, of_h);

    let bar_y = rc.top + 26;
    let track = RECT {
        left: rc.left,
        top: bar_y,
        right: rc.right,
        bottom: bar_y + SYNC_BAR_H,
    };
    round_rect_color(hdc, &track, C_PROGRESS_TRACK, C_PROGRESS_TRACK, SYNC_BAR_H / 2);
    if pct > 0 {
        let fill_w = ((track.right - track.left) * pct as i32).max(1) / 100;
        let fill = RECT {
            left: track.left,
            top: track.top,
            right: track.left + fill_w,
            bottom: track.bottom,
        };
        round_rect_color(hdc, &fill, bar_color, bar_color, SYNC_BAR_H / 2);
    }

    if !detail.is_empty() || !eta.is_empty() {
        let row2 = RECT {
            left: rc.left,
            top: track.bottom + 4,
            right: rc.right,
            bottom: rc.bottom,
        };
        let of_d = SelectObject(hdc, hf_detail);
        SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
        if !detail.is_empty() {
            draw_text_w(
                hdc,
                &row2,
                &detail,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        if !eta.is_empty() {
            draw_text_w(
                hdc,
                &row2,
                &eta,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
            );
        }
        SelectObject(hdc, of_d);
    }
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
    let status_subtitle = (*st).status_subtitle.clone();
    let hf = (*st).hfont;
    let hf_pill = (*st).hfont_b;
    let hf_caption = (*st).hfont_small;

    let sr = (*st).status_strip_rect;
    if STATUS_ROW_H > 0
        && sr.right > sr.left
        && sr.bottom > sr.top
        && !status_text.is_empty()
    {
        let syncing = (*st).sync_status_state == crate::sync::ActivityState::Checking as usize
            || (*st).sync_status_state == crate::sync::ActivityState::Syncing as usize;
        draw_status_pill_row(
            hdc,
            &sr,
            &status_text,
            &status_subtitle,
            accent_color,
            syncing,
            hf_pill,
            hf_caption,
        );
    }

    let br = (*st).bridge_rect;
    if br.right > br.left && br.bottom > br.top {
        draw_sync_bridge(hdc, &br, &*st);
    }

    let band = (*st).bridge_progress_rect;
    if band.right > band.left && band.bottom > band.top && bridge_show_sync_band(&*st) {
        draw_sync_band(hdc, &band, &*st);
    }

    let ar = (*st).activity_list_rect;
    if ar.right > ar.left && ar.bottom > ar.top {
        round_rect_color(hdc, &ar, C_INPUT_BG, C_PANEL_BORDER, 8);
    }

    let fr = (*st).sync_footer_rect;
    if fr.right > fr.left
        && fr.bottom > fr.top
        && ((*st).sync_footer_busy || (*st).sync_last_failed > 0)
    {
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
        let _ = LineTo(hdc, M + (*st).inner_w, dy);
        SelectObject(hdc, op);
        DeleteObject(hp);
    }

    let fp = (*st).footer_panel_rect;
    if fp.bottom > fp.top {
        let top_line = RECT {
            left: M,
            top: fp.top,
            right: M + (*st).inner_w,
            bottom: fp.top + 1,
        };
        fill_rect_color(hdc, &top_line, C_DIVIDER);

    }

    let _ = hf;
}

fn bridge_pc_path(st: &WndState) -> String {
    let path = st.config.watch_folder.trim();
    if path.is_empty() {
        "C:\\XDSoftware\\backups".to_string()
    } else {
        path.to_string()
    }
}

fn bridge_server_path(st: &WndState) -> String {
    customer_slug_label(st)
}

fn bridge_server_name(st: &WndState) -> String {
    let url = st.config.webdav_url.trim();
    if url.is_empty() {
        return "Server".to_string();
    }

    let without_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if host.is_empty() {
        "Server".to_string()
    } else {
        host.to_string()
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
            let br = CreateSolidBrush(COLORREF(C_INPUT_BG));
            FillRect(hdc, &RECT { left: 0, top: 0, right: w, bottom: h }, br);
            DeleteObject(br);
            let hp = CreatePen(PS_SOLID, 1, COLORREF(border_clr));
            let op = SelectObject(hdc, hp);
            let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            Rectangle(hdc, 0, 0, w, h);
            SelectObject(hdc, op);
            SelectObject(hdc, ob);
            DeleteObject(hp);
            ReleaseDC(hwnd, hdc);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}
