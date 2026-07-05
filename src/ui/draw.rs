// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_UPDATE_LINK];
const BORDERLESS_IDS: &[u16] = &[IDC_GITHUB];
const BRIDGE_BTN_IDS: &[u16] = &[
    IDC_OPEN_LOCAL_FOLDER,
    IDC_BROWSE_LOCAL,
    IDC_PAIR_DEVICE,
    IDC_REFRESH_REMOTE,
];

unsafe fn on_draw_item(lp: LPARAM) -> LRESULT {
    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
    let id = di.CtlID as u16;

    if id == IDC_ACTIVITY_LIST {
        return on_draw_activity_item(lp);
    }

    let is_blue = BLUE_IDS.contains(&id);
    let is_borderless = BORDERLESS_IDS.contains(&id);
    let pressed = (di.itemState.0 & ODS_SELECTED.0) != 0;
    let disabled = (di.itemState.0 & ODS_DISABLED.0) != 0;

    let (bg, fg, bc) = if disabled {
        (C_GREY_BTN, 0x00AAAAAA_u32, C_GREY_BORDER)
    } else if is_borderless {
        let b = if pressed { C_GREY_HOV } else { C_WIN_BG };
        (b, C_GREY_TXT, C_WIN_BG)
    } else if is_blue {
        let b = if pressed { C_BLUE_HOV } else { C_BLUE };
        (b, C_BLUE_TXT, b)
    } else {
        let b = if pressed { C_GREY_HOV } else { C_GREY_BTN };
        (b, C_GREY_TXT, C_GREY_BORDER)
    };

    let rc = di.rcItem;
    let hdc = di.hDC;

    let hbr = CreateSolidBrush(COLORREF(C_WIN_BG));
    FillRect(hdc, &rc, hbr);
    DeleteObject(hbr);

    // Draw border for non-borderless buttons
    if !is_borderless {
        let radius = if BRIDGE_BTN_IDS.contains(&id) { 8 } else { 7 };
        round_rect_color(hdc, &rc, bg, bc, radius);
    } else {
        let hbr = CreateSolidBrush(COLORREF(bg));
        FillRect(hdc, &rc, hbr);
        DeleteObject(hbr);
    }

    let len = GetWindowTextLengthW(di.hwndItem);
    if id == IDC_GITHUB {
        draw_github_icon(hdc, &rc, di.hwndItem);
    } else if len > 0 {
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(di.hwndItem, &mut buf);
        let hf = HFONT(SendMessageW(di.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut _);
        let of = SelectObject(hdc, hf);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(fg));
        let mut tr = rc;
        let pad = if BRIDGE_BTN_IDS.contains(&id) { 2 } else { 4 };
        tr.left += pad;
        tr.right -= pad;
        DrawTextW(
            hdc,
            &mut buf[..len as usize],
            &mut tr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
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

unsafe fn draw_github_icon(hdc: HDC, rc: &RECT, hwnd_item: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(GetParent(hwnd_item), GWLP_HINSTANCE) as *mut _);
    let icon = LoadIconW(hi, w!("APP_ICON_GITHUB")).unwrap_or_default();
    if icon.0.is_null() {
        return;
    }

    let size = 16;
    let x = rc.left + ((rc.right - rc.left - size) / 2);
    let y = rc.top + ((rc.bottom - rc.top - size) / 2);
    let _ = DrawIconEx(hdc, x, y, icon, size, size, 0, HBRUSH(std::ptr::null_mut()), DI_NORMAL);
}
