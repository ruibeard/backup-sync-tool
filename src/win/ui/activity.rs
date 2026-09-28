unsafe fn draw_activity_text(hdc: HDC, rc: &mut RECT, text: &str, format: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, &mut wide, rc, format);
}

fn activity_time_label_now() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let st = GetLocalTime();
        let h = st.wHour;
        let m = st.wMinute;
        let (h12, suffix) = match h {
            0 => (12, "AM"),
            1..=11 => (h, "AM"),
            12 => (12, "PM"),
            _ => (h - 12, "PM"),
        };
        format!("{h12}:{m:02} {suffix}")
    }
}

fn activity_time_for_kind(kind: ActivityKind) -> Option<String> {
    if kind == ActivityKind::Info {
        Some(activity_time_label_now())
    } else {
        None
    }
}

fn activity_row_height(row: &ActivityRow) -> i32 {
    match row.kind {
        ActivityKind::Error if row.detail.is_some() => ACTIVITY_ROW_H_ERROR,
        ActivityKind::Done | ActivityKind::Info | ActivityKind::Error => ACTIVITY_ROW_H_DONE,
        _ => ACTIVITY_ROW_H_ACTIVE,
    }
}

fn activity_icon_char(kind: ActivityKind, done: bool) -> &'static str {
    if done {
        "\u{2713}"
    } else {
        match kind {
            ActivityKind::Syncing => "\u{21C4}",
            ActivityKind::Error => "!",
            ActivityKind::Info => "i",
            _ => " ",
        }
    }
}

fn row_from_log_message(message: &str) -> Option<(Option<String>, ActivityRow)> {
    if let Some(path) = message.strip_prefix("Synced ") {
        let name = display_activity_name(path);
        let key = format!("sync:{path}");
        return Some((
            Some(key.clone()),
            ActivityRow {
                label: format!("Synced {name}"),
                kind: ActivityKind::Done,
                pct: None,
                detail: None,
                replace_key: Some(key),
                time_label: None,
            },
        ));
    }
    if let Some(path) = message.strip_prefix("Syncing ") {
        let name = display_activity_name(path);
        let key = format!("sync:{path}");
        return Some((
            Some(key.clone()),
            ActivityRow {
                label: format!("Syncing {name}"),
                kind: ActivityKind::Syncing,
                pct: None,
                detail: None,
                replace_key: Some(key),
                time_label: None,
            },
        ));
    }
    if let Some(detail) = message.strip_prefix("Could not sync ") {
        return Some((
            None,
            ActivityRow {
                label: "Item needs attention".into(),
                kind: ActivityKind::Error,
                pct: None,
                detail: Some(detail.to_string()),
                replace_key: None,
                time_label: None,
            },
        ));
    }
    if let Some(message) = message.strip_prefix("! ") {
        return Some((
            None,
            ActivityRow {
                label: message.to_string(),
                kind: ActivityKind::Info,
                pct: None,
                detail: None,
                replace_key: None,
                time_label: activity_time_for_kind(ActivityKind::Info),
            },
        ));
    }
    None
}

unsafe fn activity_list_hwnd(hwnd: HWND) -> HWND {
    GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32)
}

unsafe fn refresh_activity_listbox(hwnd: HWND) {
    let st = stmut(hwnd);
    let hlb = activity_list_hwnd(hwnd);
    SendMessageW(hlb, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
    if st.activity_rows.is_empty() && st.activity_show_empty {
        SendMessageW(hlb, LB_ADDSTRING, WPARAM(0), LPARAM(0));
    } else {
        for _ in &st.activity_rows {
            SendMessageW(hlb, LB_ADDSTRING, WPARAM(0), LPARAM(0));
        }
    }
    InvalidateRect(hlb, None, TRUE);
}

unsafe fn push_activity_row(hwnd: HWND, row: ActivityRow) {
    let st = stmut(hwnd);
    if st.activity_show_empty {
        st.activity_show_empty = false;
        st.activity_rows.clear();
    }
    st.activity_rows.insert(0, row);
    if st.activity_rows.len() > MAX_ACTIVITY_ROWS {
        st.activity_rows.truncate(MAX_ACTIVITY_ROWS);
    }
    refresh_activity_listbox(hwnd);
}

unsafe fn replace_activity_row(hwnd: HWND, replace_key: &str, row: ActivityRow) {
    let st = stmut(hwnd);
    st.activity_show_empty = false;
    if let Some(idx) = st
        .activity_rows
        .iter()
        .position(|r| r.replace_key.as_deref() == Some(replace_key))
    {
        st.activity_rows[idx] = row;
    } else {
        st.activity_rows.insert(0, row);
        if st.activity_rows.len() > MAX_ACTIVITY_ROWS {
            st.activity_rows.truncate(MAX_ACTIVITY_ROWS);
        }
    }
    refresh_activity_listbox(hwnd);
}


unsafe fn apply_activity_log(hwnd: HWND, message: &str) {
    let Some((replace_key, row)) = row_from_log_message(message) else {
        return;
    };
    if row.kind == ActivityKind::Error {
        push_activity_row(hwnd, row);
        return;
    }
    if stmut(hwnd).activity_show_empty {
        stmut(hwnd).activity_show_empty = false;
        stmut(hwnd).activity_rows.clear();
    }
    if let Some(key) = replace_key {
        replace_activity_row(hwnd, &key, row);
    } else {
        push_activity_row(hwnd, row);
    }
}

unsafe fn on_measure_item(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let mis = &mut *(lp.0 as *mut MEASUREITEMSTRUCT);
    if mis.CtlID != IDC_ACTIVITY_LIST as u32 {
        return LRESULT(0);
    }
    let st = state_ptr(hwnd);
    if st.is_null() {
        return LRESULT(0);
    }
    let rows = &(*st).activity_rows;
    let show_empty = (*st).activity_show_empty;
    mis.itemHeight = if rows.is_empty() && show_empty {
        MIN_ACTIVITY_LIST_H as u32
    } else if let Some(row) = rows.get(mis.itemID as usize) {
        activity_row_height(row) as u32
    } else {
        ACTIVITY_ROW_H_DONE as u32
    };
    LRESULT(1)
}

unsafe fn activity_content_right(rc: &RECT) -> i32 {
    let scroll_w = GetSystemMetrics(SM_CXVSCROLL);
    rc.right.saturating_sub(scroll_w.max(16))
}

unsafe fn draw_activity_row(
    hdc: HDC,
    rc: &RECT,
    row: Option<&ActivityRow>,
    empty: bool,
    anim_frame: usize,
    hf_label: HFONT,
    hf_status: HFONT,
) {
    let hbr = CreateSolidBrush(COLORREF(C_STATUS_BG));
    FillRect(hdc, rc, hbr);
    DeleteObject(hbr);

    let content_right = activity_content_right(rc);

    if empty {
        let of = SelectObject(hdc, hf_label);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(0x00999999));
        let mut tr = *rc;
        tr.right = content_right;
        draw_activity_text(
            hdc,
            &mut tr,
            "No recent activity",
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(hdc, of);
        return;
    }

    let Some(row) = row else {
        return;
    };

    let done = row.kind == ActivityKind::Done;
    let is_error = row.kind == ActivityKind::Error;
    let show_bar = row.kind == ActivityKind::Syncing;
    let icon = activity_icon_char(row.kind, done);
    SetBkMode(hdc, TRANSPARENT);

    let mut top_line = *rc;
    top_line.left = rc.left + ACTIVITY_PAD_LEFT;
    top_line.right = content_right - ACTIVITY_PAD_RIGHT;
    top_line.top += 5;
    top_line.bottom = if show_bar {
        rc.bottom - 9
    } else if is_error && row.detail.is_some() {
        rc.bottom - 14
    } else {
        rc.bottom - 5
    };

    let status_right = top_line.right;
    let status_left = status_right - ACTIVITY_STATUS_W;

    let mut icon_rc = top_line;
    icon_rc.right = icon_rc.left + 16;
    let of_icon = SelectObject(hdc, hf_label);
    SetTextColor(
        hdc,
        COLORREF(if is_error {
            C_RED
        } else if done {
            C_GREEN
        } else {
            C_STATUS_MUTED
        }),
    );
    draw_activity_text(
        hdc,
        &mut icon_rc,
        icon,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );

    let mut label_rc = top_line;
    label_rc.left += 24;
    label_rc.right = if show_bar || done || is_error || row.time_label.is_some() {
        status_left - 4
    } else {
        top_line.right
    };
    SetTextColor(
        hdc,
        COLORREF(if row.kind == ActivityKind::Info {
            C_STATUS_MUTED
        } else {
            C_LABEL
        }),
    );
    draw_activity_text(
        hdc,
        &mut label_rc,
        &row.label,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );

    if is_error {
        if let Some(detail) = row.detail.as_deref() {
            let of_detail = SelectObject(hdc, hf_status);
            SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
            let mut detail_rc = *rc;
            detail_rc.left = label_rc.left;
            detail_rc.right = top_line.right;
            detail_rc.top = top_line.bottom;
            detail_rc.bottom = rc.bottom - 4;
            draw_activity_text(
                hdc,
                &mut detail_rc,
                detail,
                DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            SelectObject(hdc, of_detail);
        }
        let of_status = SelectObject(hdc, hf_status);
        SetTextColor(hdc, COLORREF(C_RED));
        let mut fail_rc = top_line;
        fail_rc.left = status_left;
        fail_rc.right = status_right;
        draw_activity_text(
            hdc,
            &mut fail_rc,
            "Failed",
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, of_status);
    } else if show_bar {
        let status = if done {
            Some("Done".to_string())
        } else if let Some(pct) = row.pct {
            Some(format!("{pct}%"))
        } else {
            None
        };
        if let Some(status) = status {
            let of_status = SelectObject(hdc, hf_status);
            SetTextColor(hdc, COLORREF(if done { C_GREEN } else { C_STATUS_MUTED }));
            let mut pct_rc = top_line;
            pct_rc.left = status_left;
            pct_rc.right = status_right;
            draw_activity_text(
                hdc,
                &mut pct_rc,
                &status,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(hdc, of_status);
        }

        let bar_left = top_line.left;
        let bar_right = top_line.right;
        let bar_top = rc.bottom - 7;
        let bar_bottom = rc.bottom - 4;
        let track = RECT {
            left: bar_left,
            top: bar_top,
            right: bar_right,
            bottom: bar_bottom,
        };
        let br_track = CreateSolidBrush(COLORREF(C_ACTIVITY_TRACK));
        FillRect(hdc, &track, br_track);
        DeleteObject(br_track);

        let inner_w = (bar_right - bar_left).max(1);
        let fill_w = if let Some(pct) = row.pct {
            (inner_w * pct as i32) / 100
        } else {
            let chunk = (inner_w / 3).max(8);
            let travel = inner_w - chunk;
            let offset = if travel > 0 {
                ((anim_frame as i32 * chunk / 2) % travel).max(0)
            } else {
                0
            };
            chunk + offset
        };
        if fill_w > 0 {
            let fill = RECT {
                left: bar_left,
                top: bar_top,
                right: bar_left + fill_w.min(inner_w),
                bottom: bar_bottom,
            };
            let br_fill = CreateSolidBrush(COLORREF(C_PROGRESS_MINI));
            FillRect(hdc, &fill, br_fill);
            DeleteObject(br_fill);
        }
    } else if done {
        let of_status = SelectObject(hdc, hf_status);
        SetTextColor(hdc, COLORREF(C_GREEN));
        let mut done_rc = top_line;
        done_rc.left = status_left;
        done_rc.right = status_right;
        draw_activity_text(
            hdc,
            &mut done_rc,
            "Done",
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, of_status);
    } else if let Some(time) = row.time_label.as_deref() {
        let of_status = SelectObject(hdc, hf_status);
        SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
        let mut time_rc = top_line;
        time_rc.left = status_left;
        time_rc.right = status_right;
        draw_activity_text(
            hdc,
            &mut time_rc,
            time,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, of_status);
    }

    SelectObject(hdc, of_icon);

    let hp = CreatePen(PS_SOLID, 1, COLORREF(0x00F0F0F0));
    let op = SelectObject(hdc, hp);
    let _ = MoveToEx(hdc, rc.left, rc.bottom - 1, None);
    let _ = LineTo(hdc, rc.right, rc.bottom - 1);
    SelectObject(hdc, op);
    DeleteObject(hp);
}

unsafe fn on_draw_activity_item(lp: LPARAM) -> LRESULT {
    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
    if di.CtlID != IDC_ACTIVITY_LIST as u32 {
        return LRESULT(0);
    }

    let parent = GetParent(di.hwndItem);
    let st = state_ptr(parent);
    if st.is_null() {
        return LRESULT(0);
    }

    let rows = &(*st).activity_rows;
    let empty = rows.is_empty() && (*st).activity_show_empty;
    let row = if empty {
        None
    } else {
        rows.get(di.itemID as usize)
    };

    draw_activity_row(
        di.hDC,
        &di.rcItem,
        row,
        empty && di.itemID == 0,
        (*st).sync_anim_frame,
        (*st).hfont_activity,
        (*st).hfont_small,
    );
    LRESULT(1)
}
