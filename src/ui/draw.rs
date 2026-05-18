// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_SAVE, IDC_UPDATE_LINK];
const BORDERLESS_IDS: &[u16] = &[IDC_BROWSE_LOCAL, IDC_GITHUB];
const FOLDER_IDS: &[u16] = &[IDC_BROWSE_LOCAL];
const UPDATE_IDS: &[u16] = &[IDC_UPDATE_LINK];
const GITHUB_IDS: &[u16] = &[IDC_GITHUB];

unsafe fn on_draw_item(lp: LPARAM) -> LRESULT {
    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
    let id = di.CtlID as u16;

    let is_blue = BLUE_IDS.contains(&id);
    let is_borderless = BORDERLESS_IDS.contains(&id);
    let pressed = (di.itemState.0 & ODS_SELECTED.0) != 0;
    let disabled = (di.itemState.0 & ODS_DISABLED.0) != 0;

    let (bg, fg, bc) = if disabled {
        (C_GREY_BTN, 0x00AAAAAA_u32, C_GREY_BORDER)
    } else if is_blue {
        let b = if pressed { C_BLUE_HOV } else { C_BLUE };
        (b, C_BLUE_TXT, b)
    } else {
        let b = if pressed { C_GREY_HOV } else { C_GREY_BTN };
        (b, C_GREY_TXT, C_GREY_BORDER)
    };

    let rc = di.rcItem;
    let hdc = di.hDC;

    let hbr = CreateSolidBrush(COLORREF(bg));
    FillRect(hdc, &rc, hbr);
    DeleteObject(hbr);

    // Draw border for non-borderless buttons
    if !is_borderless {
        let hp = CreatePen(PS_SOLID, 1, COLORREF(bc));
        let op = SelectObject(hdc, hp);
        let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 5, 5);
        SelectObject(hdc, op);
        SelectObject(hdc, ob);
        DeleteObject(hp);
    }

    let len = GetWindowTextLengthW(di.hwndItem);
    let is_folder = FOLDER_IDS.contains(&id);
    let is_update = UPDATE_IDS.contains(&id);
    let is_github = GITHUB_IDS.contains(&id);

    if is_folder {
        // Draw a small folder icon via GDI
        draw_folder_icon(hdc, &rc, fg);
    } else if is_update {
        // Draw a download arrow icon via GDI
        draw_download_icon(hdc, &rc, fg);
    } else if is_github {
        draw_github_icon(hdc, &rc, di.hwndItem);
    } else if len > 0 {
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(di.hwndItem, &mut buf);
        let hf = HFONT(SendMessageW(di.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as isize);
        let of = SelectObject(hdc, hf);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(fg));
        let mut tr = rc;
        tr.left += 4;
        tr.right -= 4;
        DrawTextW(
            hdc,
            &mut buf[..len as usize],
            &mut tr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(hdc, of);
    }

    if (di.itemState.0 & ODS_FOCUS.0) != 0 {
        let mut fr = rc;
        fr.left += 3;
        fr.top += 3;
        fr.right -= 3;
        fr.bottom -= 3;
        DrawFocusRect(hdc, &fr);
    }
    LRESULT(1)
}

/// Draw a small folder icon centred in the given rect.
/// Uses GDI primitives: filled rectangle body + small tab on top-left.
unsafe fn draw_folder_icon(hdc: HDC, rc: &RECT, _text_clr: u32) {
    let cx = (rc.left + rc.right) / 2;
    let cy = (rc.top + rc.bottom) / 2;

    // Folder dimensions
    let fw = 14i32; // total width
    let fh = 10i32; // body height
    let tab_w = 6i32; // tab width
    let tab_h = 3i32; // tab height

    let x0 = cx - fw / 2;
    let y0 = cy - (fh + tab_h) / 2 + tab_h;

    // Draw tab (small rectangle on top-left)
    let tab_brush = CreateSolidBrush(COLORREF(C_FOLDER_FILL));
    let tab_pen = CreatePen(PS_SOLID, 1, COLORREF(C_FOLDER_LINE));
    let op = SelectObject(hdc, tab_pen);
    let ob = SelectObject(hdc, tab_brush);

    // Tab trapezoid as a simple rect
    Rectangle(hdc, x0, y0 - tab_h, x0 + tab_w, y0 + 1);

    // Body
    Rectangle(hdc, x0, y0, x0 + fw, y0 + fh);

    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(tab_brush);
    DeleteObject(tab_pen);
}

/// Draw a download-arrow icon centred in the given rect.
/// Arrow pointing down with a horizontal line (tray) below it.
unsafe fn draw_download_icon(hdc: HDC, rc: &RECT, clr: u32) {
    let cx = (rc.left + rc.right) / 2;
    let cy = (rc.top + rc.bottom) / 2;

    let hp = CreatePen(PS_SOLID, 2, COLORREF(clr));
    let op = SelectObject(hdc, hp);

    // Vertical line (shaft of arrow)
    MoveToEx(hdc, cx, cy - 5, None);
    LineTo(hdc, cx, cy + 3);

    // Arrowhead: two diagonal lines from tip
    MoveToEx(hdc, cx - 3, cy, None);
    LineTo(hdc, cx, cy + 3);
    MoveToEx(hdc, cx + 3, cy, None);
    LineTo(hdc, cx, cy + 3);

    // Tray / base line
    MoveToEx(hdc, cx - 5, cy + 6, None);
    LineTo(hdc, cx + 6, cy + 6);

    SelectObject(hdc, op);
    DeleteObject(hp);
}

unsafe fn draw_github_icon(hdc: HDC, rc: &RECT, hwnd_item: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(GetParent(hwnd_item), GWLP_HINSTANCE) as isize);
    let icon = LoadIconW(hi, w!("APP_ICON_GITHUB")).unwrap_or_default();
    if icon.0 == 0 {
        return;
    }

    let size = 16;
    let x = rc.left + ((rc.right - rc.left - size) / 2);
    let y = rc.top + ((rc.bottom - rc.top - size) / 2);
    let _ = DrawIconEx(hdc, x, y, icon, size, size, 0, HBRUSH(0), DI_NORMAL);
}

