// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_SAVE, IDC_UPDATE_LINK];
const BORDERLESS_IDS: &[u16] = &[IDC_GITHUB];
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
    let is_github = GITHUB_IDS.contains(&id);

    if is_github {
        draw_github_icon(hdc, &rc, di.hwndItem);
    } else if len > 0 {
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(di.hwndItem, &mut buf);
        let hf = HFONT(SendMessageW(di.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut _);
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
