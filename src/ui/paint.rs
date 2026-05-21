// ── Background paint ──────────────────────────────────────────────────────────
const BRIDGE_PC_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/bridge-pc.png"));
const BRIDGE_CLOUD_PNG: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/bridge-cloud.png"));

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
        png_bytes_to_hbitmap(BRIDGE_CLOUD_PNG, target_px),
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
    let badge = 16;
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

unsafe fn draw_sync_bridge(hdc: HDC, br: &RECT, st: &WndState) {
    round_rect_color(hdc, br, C_BRIDGE_BG, C_BRIDGE_BORDER, 8);

    let inner_w = br.right - br.left;
    let layout = bridge_layout_at(br.top, inner_w);
    let hf_name = st.hfont_bridge_name;
    let hf_path = st.hfont_bridge_path;
    let hf_mid = st.hfont_bridge_mid;
    let hf_caption = st.hfont_small;

    let syncing = st.sync_status_state == crate::sync::ActivityState::Checking as usize
        || st.sync_status_state == crate::sync::ActivityState::Syncing as usize;
    let flow_idle = !syncing && st.bridge_sync_head == "All synced";

    let of_name = SelectObject(hdc, hf_name);
    SetBkMode(hdc, TRANSPARENT);

    draw_bridge_icon_png(hdc, &layout.left_ico, st.bridge_icon_pc);
    draw_bridge_icon_png(hdc, &layout.right_ico, st.bridge_icon_cloud);
    draw_bridge_icon_badge(hdc, &layout.right_ico, st.bridge_conn_ok, st.hfont_small);

    draw_bridge_node_name(hdc, &layout.left_name, "This PC", hf_name, None);
    draw_bridge_node_name(hdc, &layout.right_name, "Server", hf_name, None);

    let of_s = SelectObject(hdc, hf_path);
    SetTextColor(hdc, COLORREF(C_BRIDGE_PATH_TXT));
    draw_text_w(
        hdc,
        &layout.left_path,
        &bridge_pc_path(st),
        DT_CENTER | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    draw_text_w(
        hdc,
        &layout.right_path,
        &bridge_server_path(st),
        DT_CENTER | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
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
        &layout.right_conn,
        &st.bridge_conn_label,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SelectObject(hdc, of_s);
    SelectObject(hdc, of_name);

    draw_bridge_mid_h6(
        hdc,
        &layout.mid,
        &st.bridge_sync_head,
        &st.bridge_sync_meta,
        flow_idle,
        st.sync_anim_frame,
        syncing,
        hf_mid,
        hf_caption,
    );

    if bridge_show_progress_block(st) {
        draw_bridge_batch_progress(hdc, &st.bridge_progress_rect, st, hf_path, hf_mid);
    }
}

unsafe fn draw_bridge_mid_h6(
    hdc: HDC,
    mid: &RECT,
    head: &str,
    meta: &str,
    flow_idle: bool,
    anim_frame: usize,
    syncing: bool,
    hf_head: HFONT,
    hf_meta: HFONT,
) {
    let head_h = 14;
    let gap = 4;
    let block_top = mid.top + 6;

    let head_color = if syncing {
        C_BRIDGE_SYNC_HEAD_ACTIVE
    } else if flow_idle {
        C_BRIDGE_SYNC_HEAD_OK
    } else {
        C_BRIDGE_SYNC_HEAD_IDLE
    };
    let of_h = SelectObject(hdc, hf_head);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(head_color));
    let head_rc = RECT {
        left: mid.left,
        top: block_top,
        right: mid.right,
        bottom: block_top + head_h,
    };
    draw_text_w(
        hdc,
        &head_rc,
        head,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
    SelectObject(hdc, of_h);

    let line_y = block_top + head_h + gap;
    draw_bridge_flow(hdc, mid.left, mid.right, line_y, flow_idle, anim_frame, syncing);

    let of_m = SelectObject(hdc, hf_meta);
    SetTextColor(hdc, COLORREF(C_BRIDGE_PATH_TXT));
    let meta_rc = RECT {
        left: mid.left,
        top: line_y + BRIDGE_FLOW_H + gap,
        right: mid.right,
        bottom: (line_y + BRIDGE_FLOW_H + gap + BRIDGE_META_H).min(mid.bottom),
    };
    let meta_text = meta.replace("\r\n", "\n");
    draw_text_w(
        hdc,
        &meta_rc,
        &meta_text,
        DT_CENTER | DT_TOP | DT_WORDBREAK | DT_NOPREFIX | DT_EDITCONTROL,
    );
    SelectObject(hdc, of_m);
}

unsafe fn draw_bridge_batch_progress(
    hdc: HDC,
    rc: &RECT,
    st: &WndState,
    hf_detail: HFONT,
    hf_pct: HFONT,
) {
    if rc.bottom <= rc.top || rc.right <= rc.left {
        return;
    }

    let syncing = bridge_syncing_progress(st);
    let all_synced = st.bridge_sync_head == "All synced";
    let (pct, bar_color, detail, eta, verb, pct_color) = if syncing {
        let done = st.sync_progress_done.min(st.sync_progress_total);
        let total = st.sync_progress_total;
        let pct = if total > 0 {
            (done * 100) / total
        } else {
            0
        };
        let file = st
            .activity_rows
            .iter()
            .find(|row| row.kind == ActivityKind::Uploading || row.kind == ActivityKind::Downloading)
            .map(|row| row.label.as_str())
            .unwrap_or("...");
        let detail = if total > 0 {
            format!("{done} of {total} files · {file}")
        } else {
            file.to_string()
        };
        let eta = st
            .sync_status_text
            .split("ETA ")
            .nth(1)
            .and_then(|s| s.split('\u{00B7}').next())
            .map(|eta| format!("ETA {eta}"))
            .unwrap_or_default();
        let verb = if st
            .activity_rows
            .iter()
            .any(|row| row.kind == ActivityKind::Downloading)
        {
            "Downloading"
        } else {
            "Uploading"
        };
        (
            pct,
            C_BLUE,
            detail,
            eta,
            verb.to_string(),
            C_BRIDGE_SYNC_HEAD_ACTIVE,
        )
    } else if all_synced {
        (
            100,
            C_GREEN,
            String::new(),
            String::new(),
            "Up to date".to_string(),
            C_BRIDGE_SYNC_HEAD_OK,
        )
    } else if st.bridge_sync_head == "Checking" {
        (
            0,
            C_PROGRESS_TRACK,
            String::new(),
            String::new(),
            "Checking".to_string(),
            C_BRIDGE_SYNC_HEAD_IDLE,
        )
    } else {
        (
            0,
            C_PROGRESS_TRACK,
            String::new(),
            String::new(),
            String::new(),
            C_BRIDGE_SYNC_HEAD_IDLE,
        )
    };

    let divider = RECT {
        left: rc.left,
        top: rc.top,
        right: rc.right,
        bottom: rc.top + 1,
    };
    fill_rect_color(hdc, &divider, 0x00F5E8EE);

    let row1 = RECT {
        left: rc.left,
        top: rc.top + 6,
        right: rc.right,
        bottom: rc.top + 20,
    };
    let of_d = SelectObject(hdc, hf_detail);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(C_LABEL));
    if !detail.is_empty() {
        draw_text_w(
            hdc,
            &row1,
            &detail,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
    let pct_text = format!("{pct}%");
    SetTextColor(hdc, COLORREF(pct_color));
    let of_p = SelectObject(hdc, hf_pct);
    if pct > 0 || all_synced {
        draw_text_w(
            hdc,
            &row1,
            &pct_text,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
    SelectObject(hdc, of_d);

    let bar_y = rc.top + 22;
    let track = RECT {
        left: rc.left,
        top: bar_y,
        right: rc.right,
        bottom: bar_y + BRIDGE_PROGRESS_H,
    };
    round_rect_color(hdc, &track, C_PROGRESS_TRACK, C_PROGRESS_TRACK, 2);
    if pct > 0 {
        let fill_w = ((track.right - track.left) * pct as i32).max(1) / 100;
        let fill = RECT {
            left: track.left,
            top: track.top,
            right: track.left + fill_w,
            bottom: track.bottom,
        };
        round_rect_color(hdc, &fill, bar_color, bar_color, 2);
    }

    let row2 = RECT {
        left: rc.left,
        top: (rc.bottom - 16).max(bar_y + BRIDGE_PROGRESS_H + 4),
        right: rc.right,
        bottom: rc.bottom - 2,
    };
    SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
    if !eta.is_empty() {
        draw_text_w(
            hdc,
            &row2,
            &eta,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
    if !verb.is_empty() {
        draw_text_w(
            hdc,
            &row2,
            &verb,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
        );
    }
    SelectObject(hdc, of_p);
}

unsafe fn draw_bridge_flow(
    hdc: HDC,
    left: i32,
    right: i32,
    line_y: i32,
    flow_idle: bool,
    anim_frame: usize,
    syncing: bool,
) {
    let line_w = right - left;
    let track = RECT {
        left,
        top: line_y,
        right,
        bottom: line_y + BRIDGE_FLOW_H,
    };
    round_rect_color(hdc, &track, C_FLOW_TRACK, C_FLOW_TRACK, 2);

    let flow_w = if flow_idle {
        line_w
    } else if syncing {
        (line_w * 45) / 100
    } else {
        0
    };
    if flow_w <= 0 {
        return;
    }
    let flow_color = if flow_idle { C_GREEN } else { C_FLOW_SYNC };
    let offset = if flow_idle || !syncing {
        0
    } else {
        let span = (line_w - flow_w + 1).max(1) as usize;
        ((anim_frame * 7) % span) as i32
    };
    let flow = RECT {
        left: track.left + offset,
        top: track.top,
        right: track.left + offset + flow_w,
        bottom: track.bottom,
    };
    round_rect_color(hdc, &flow, flow_color, flow_color, 2);
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

    let ar = (*st).activity_list_rect;
    if ar.right > ar.left && ar.bottom > ar.top {
        fill_rect_color(hdc, &ar, C_INPUT_BG);
        frame_rect_color(hdc, &ar, C_PANEL_BORDER);
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
    if !is_paired(&st.config) {
        return "Not paired".to_string();
    }
    if let Some(name) = st
        .detected_customer
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    let cfg = &st.config;
    if !cfg.device_token_enc.trim().is_empty() {
        return friendly_remote_name(&cfg.remote_folder);
    }
    if st.remote_folder_from_xd && !cfg.remote_folder.trim().is_empty() {
        if let Some(customer) = st.detected_customer.as_deref() {
            let trimmed = customer.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        return friendly_remote_name(&cfg.remote_folder);
    }
    "Waiting for pairing approval".to_string()
}

fn friendly_remote_name(folder: &str) -> String {
    let trimmed = folder.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..]
            .join(" ")
            .replace('-', " ")
    } else {
        trimmed.to_string()
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
