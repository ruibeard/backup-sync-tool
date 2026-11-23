unsafe fn draw_activity_text(hdc: HDC, rc: &mut RECT, text: &str, format: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, &mut wide, rc, format);
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Local time minus UTC, in seconds.
fn local_offset_secs() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let secs = |t: windows::Win32::Foundation::SYSTEMTIME| {
        days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64) * 86_400
            + t.wHour as i64 * 3600
            + t.wMinute as i64 * 60
            + t.wSecond as i64
    };
    unsafe { secs(GetLocalTime()) - secs(GetSystemTime()) }
}

/// "14:05" for today, "Yesterday 23:10" for older rows.
fn activity_time_label(at: u64) -> String {
    let offset = local_offset_secs();
    let local = at as i64 + offset;
    let today = (unix_now() as i64 + offset).div_euclid(86_400);
    let day = local.div_euclid(86_400);
    let mins = local.rem_euclid(86_400) / 60;
    let clock = format!("{:02}:{:02}", mins / 60, mins % 60);
    if day == today {
        clock
    } else {
        format!("Yesterday {clock}")
    }
}

/// Rolling log of finished rows: `<unix secs>\t<activity line>`.
fn activity_history_path() -> std::path::PathBuf {
    crate::paths::app_support_dir().join("activity.log")
}

/// Rows written since the file was last trimmed to 24 hours.
static ACTIVITY_APPENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Lines from the last 24 hours, oldest first.
fn read_activity_history() -> Vec<(u64, String)> {
    let cutoff = unix_now().saturating_sub(ACTIVITY_KEEP_SECS);
    let text = std::fs::read_to_string(activity_history_path()).unwrap_or_default();
    let mut lines: Vec<(u64, String)> = text
        .lines()
        .filter_map(|l| {
            let (at, msg) = l.split_once('\t')?;
            Some((at.parse().ok()?, msg.to_string()))
        })
        .filter(|(at, _)| *at >= cutoff)
        .collect();
    let keep = lines.len().saturating_sub(MAX_ACTIVITY_ROWS);
    lines.drain(..keep);
    lines
}

/// Rewrite the file with the last 24 hours only.
fn trim_activity_history(lines: &[(u64, String)]) {
    let path = activity_history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = lines.iter().map(|(at, msg)| format!("{at}\t{msg}\n")).collect();
    let _ = std::fs::write(path, text);
    ACTIVITY_APPENDS.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn append_activity_history(at: u64, message: &str) {
    use std::io::Write;
    let path = activity_history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{at}\t{}", message.replace(['\n', '\r'], " "));
    }
    if ACTIVITY_APPENDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1000 {
        trim_activity_history(&read_activity_history());
    }
}

/// Fill the list from the file at startup, newest first.
unsafe fn load_activity_history(hwnd: HWND) {
    let lines = read_activity_history();
    trim_activity_history(&lines);
    for (at, msg) in &lines {
        if let Some((key, row)) = row_from_log_message(msg, *at) {
            insert_activity_row(stmut(hwnd), key.as_deref(), row);
        }
    }
    refresh_activity_listbox(hwnd);
}

fn activity_row_height(row: &ActivityRow) -> i32 {
    match row.kind {
        ActivityKind::Error if row.detail.is_some() => ACTIVITY_ROW_H_ERROR,
        ActivityKind::Done | ActivityKind::Info | ActivityKind::Error => ACTIVITY_ROW_H_DONE,
        _ => ACTIVITY_ROW_H_ACTIVE,
    }
}

fn activity_icon_char(row: &ActivityRow) -> &'static str {
    match row.kind {
        ActivityKind::Error => "!",
        ActivityKind::Info => "i",
        // Up, down or delete arrow: green when done, grey while it moves.
        ActivityKind::Syncing | ActivityKind::Done => row.icon,
    }
}

/// `at` is when the line arrived; rows still running get no time.
fn row_from_log_message(message: &str, at: u64) -> Option<(Option<String>, ActivityRow)> {
    use crate::app::Activity;
    match crate::app::parse_activity(message)? {
        Activity::File { dir, done, path } => {
            let key = format!("file:{path}");
            Some((
                Some(key.clone()),
                ActivityRow {
                    label: dir.label(done, display_activity_name(path)),
                    kind: if done {
                        ActivityKind::Done
                    } else {
                        ActivityKind::Syncing
                    },
                    pct: None,
                    detail: None,
                    replace_key: Some(key),
                    at: done.then_some(at),
                    path: Some(path.to_string()),
                    icon: dir.icon(),
                },
            ))
        }
        Activity::Waiting { dir, path } => {
            let key = format!("file:{path}");
            Some((
                Some(key.clone()),
                ActivityRow {
                    label: format!("{} changed, will retry", display_activity_name(path)),
                    kind: ActivityKind::Syncing,
                    pct: None,
                    detail: None,
                    replace_key: Some(key),
                    at: None,
                    path: Some(path.to_string()),
                    icon: dir.icon(),
                },
            ))
        }
        Activity::Failed { path, reason } => {
            let key = format!("file:{path}");
            let name = display_activity_name(path);
            Some((
                Some(key.clone()),
                ActivityRow {
                    label: if reason.is_empty() {
                        format!("Could not sync {name}")
                    } else {
                        format!("Could not sync {name}: {reason}")
                    },
                    kind: ActivityKind::Error,
                    pct: None,
                    detail: None,
                    replace_key: Some(key),
                    at: Some(at),
                    path: Some(path.to_string()),
                    icon: "!",
                },
            ))
        }
        Activity::Info(message) => Some((
            None,
            ActivityRow {
                label: message.to_string(),
                kind: ActivityKind::Info,
                pct: None,
                detail: None,
                replace_key: None,
                at: Some(at),
                path: None,
                icon: "i",
            },
        )),
    }
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
    let _ = InvalidateRect(hlb, None, TRUE);
}

/// Newest first. A finished row moves to the top, so the list reads in the
/// order things ended; a running row updates in place.
fn insert_activity_row(st: &mut WndState, key: Option<&str>, row: ActivityRow) {
    if st.activity_show_empty {
        st.activity_show_empty = false;
        st.activity_rows.clear();
    }
    let old = key.and_then(|k| {
        st.activity_rows
            .iter()
            .position(|r| r.replace_key.as_deref() == Some(k))
    });
    match old {
        Some(idx) if row.at.is_none() => st.activity_rows[idx] = row,
        Some(idx) => {
            st.activity_rows.remove(idx);
            st.activity_rows.insert(0, row);
        }
        None => st.activity_rows.insert(0, row),
    }
    // Drop finished rows older than 24 hours; running rows stay.
    let cutoff = unix_now().saturating_sub(ACTIVITY_KEEP_SECS);
    st.activity_rows.retain(|r| r.at.is_none_or(|at| at >= cutoff));
    st.activity_rows.truncate(MAX_ACTIVITY_ROWS);
}

unsafe fn apply_activity_log(hwnd: HWND, message: &str) {
    let at = unix_now();
    let Some((key, row)) = row_from_log_message(message, at) else {
        return;
    };
    if row.at.is_some() {
        append_activity_history(at, message);
    }
    insert_activity_row(stmut(hwnd), key.as_deref(), row);
    refresh_activity_listbox(hwnd);
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
    let _ = DeleteObject(hbr);

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
    let icon = activity_icon_char(row);
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
    label_rc.right = if show_bar || row.at.is_some() {
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

    if show_bar {
        let status = if let Some(pct) = row.pct {
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
        let _ = DeleteObject(br_track);

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
            let _ = DeleteObject(br_fill);
        }
    } else if let Some(at) = row.at {
        let time = activity_time_label(at);
        let of_status = SelectObject(hdc, hf_status);
        SetTextColor(hdc, COLORREF(C_STATUS_MUTED));
        let mut time_rc = top_line;
        time_rc.left = status_left;
        time_rc.right = status_right;
        draw_activity_text(
            hdc,
            &mut time_rc,
            &time,
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
    let _ = DeleteObject(hp);
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

/// Double-click on an activity row: open that file, or its folder if the file is gone.
unsafe fn open_activity_row(hwnd: HWND) {
    let hlb = activity_list_hwnd(hwnd);
    let idx = SendMessageW(hlb, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    if idx < 0 {
        return;
    }
    let st = stmut(hwnd);
    let Some(rel) = st
        .activity_rows
        .get(idx as usize)
        .and_then(|r| r.path.clone())
    else {
        return;
    };
    let root = st.config.watch_folder.trim().to_string();
    if root.is_empty() {
        return;
    }
    let mut target = Path::new(&root).join(rel.replace('/', "\\"));
    while !target.exists() {
        match target.parent() {
            Some(p) if p.starts_with(&root) => target = p.to_path_buf(),
            _ => return,
        }
    }
    let _ = windows::Win32::UI::Shell::ShellExecuteW(
        Some(hwnd),
        w!("open"),
        &hstring(&target.to_string_lossy()),
        None,
        None,
        SW_SHOWNORMAL,
    );
}
