// ui.rs — Win32 main window
//
// Design (post-redesign):
//   Window bg:       #F0F0F0
//   No card boxes — sections separated by spacing + section headings only
//   Section headings: #888888, Segoe UI 10pt SemiBold, ALL CAPS
//   Field labels:     above inputs, left-aligned
//   Inputs:          white bg, 1px #CCCCCC border (blue on focus)
//   Password field:  eye icon drawn inside right padding of edit subclass
//   Connect/Save:    blue #2B4FA3, white text; Save is primary, Close secondary
//   Browse/Close:    #E8E8E8 grey, #333333 text
//   Status dot:      inline next to URL input (redrawn in WM_PAINT)
//   Bottom bar:      version (small, grey) left; checkboxes left; SAVE / CLOSE right
//   Spacing:         PAD=8, GAP=12, SECT=20 rhythm

use crate::config::Config;
use crate::secret;
use crate::tray;
use crate::webdav;
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, ShellExecuteW,
    SetWindowSubclass, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── Colours  0x00BBGGRR ──────────────────────────────────────────────────────
const C_WIN_BG: u32 = 0x00F0F0F0;
const C_SECT_BG: u32 = 0x00F0F0F0; // no card box — same as window bg
const C_LABEL: u32 = 0x00333333;
const C_HDR: u32 = 0x00888888;
const C_SUBTITLE: u32 = 0x00999999;
const C_INPUT_BG: u32 = 0x00FFFFFF;
const C_INPUT_BORDER: u32 = 0x00CCCCCC;
const C_INPUT_FOCUS: u32 = 0x00A34F2B;
const C_BLUE: u32 = 0x00A34F2B;
const C_BLUE_HOV: u32 = 0x007A3A1E;
const C_BLUE_TXT: u32 = 0x00FFFFFF;
const C_GREY_BTN: u32 = 0x00E8E8E8;
const C_GREY_HOV: u32 = 0x00D8D8D8;
const C_GREY_TXT: u32 = 0x00333333;
const C_GREY_BORDER: u32 = 0x00BBBBBB;
const C_GREEN: u32 = 0x00287A28; // connected
const C_RED: u32 = 0x000000CC; // not connected
const C_EYE: u32 = 0x00AAAAAA; // eye icon glyph colour
const C_DIVIDER: u32 = 0x00E0E0E0; // section separator line

// ── Control IDs ──────────────────────────────────────────────────────────────
const IDC_WATCH_FOLDER: u16 = 101;
const IDC_BROWSE_LOCAL: u16 = 102;
const IDC_URL: u16 = 103;
const IDC_USERNAME: u16 = 104;
const IDC_PASSWORD: u16 = 105;
const IDC_REMOTE_FOLDER: u16 = 106;
const IDC_BROWSE_REMOTE: u16 = 107;
const IDC_CONNECT: u16 = 108;
const IDC_STATUS_TEXT: u16 = 109;
const IDC_SAVE: u16 = 110;
const IDC_CLOSE: u16 = 111;
const IDC_UPDATE: u16 = 112;
const IDC_VERSION: u16 = 113;
const IDC_ACTIVITY_LIST: u16 = 114;
const IDC_START_WINDOWS: u16 = 115;
const IDC_SYNC_REMOTE: u16 = 116;
// IDC_SHOW_PASSWORD (117) removed — eye icon is now drawn inside the edit subclass
const IDC_SUBTITLE: u16 = 118;
const IDC_REMOTE_LABEL: u16 = 119;
const IDC_REPO: u16 = 120;

const WM_APP_LOG: u32 = WM_APP + 10;
const WM_APP_CONNECTED: u32 = WM_APP + 11;
const WM_APP_UPDATE: u32 = WM_APP + 12;

const SS_LEFT: u32 = 0x0000;

pub const CLASS_NAME: PCWSTR = w!("BackupSyncToolWnd");
const REPO_URL: &str = "https://github.com/ruibeard/backup-sync-tool";

// ── Layout — 8/12/20 rhythm ──────────────────────────────────────────────────
const WIN_W: i32 = 460; // client width (slightly narrower, cleaner)
const M: i32 = 16; // outer margin
const PAD: i32 = 8; // small gap (between items in same group)
const GAP: i32 = 12; // medium gap (between rows)
const SECT: i32 = 20; // section separator gap
const INP_H: i32 = 26; // input height
const BTN_H: i32 = 30; // bottom-bar primary button height
const CONN_H: i32 = 26; // Connect button height (matches INP_H)
const HDR_H: i32 = 20; // section heading height
const LBL_H: i32 = 18; // label text height
const BROWSE_W: i32 = 68; // Browse button width
const INNER_W: i32 = WIN_W - M * 2; // usable inner width
                                    // Eye icon toggle zone inside the password edit right padding
const EYE_ZONE_W: i32 = 26; // pixels from right edge of edit that count as eye click

// ── Window state ──────────────────────────────────────────────────────────────
struct WndState {
    config: Config,
    password_plain: String,
    sync_engine: Option<crate::sync::SyncEngine>,
    update_url: Option<String>,
    connected: bool,
    /// True when URL/username/password have been edited since the last save/connect
    creds_dirty: bool,
    #[allow(dead_code)]
    hfont: HFONT,
    #[allow(dead_code)]
    hfont_hdr: HFONT,
    #[allow(dead_code)]
    hfont_b: HFONT,
    #[allow(dead_code)]
    hfont_small: HFONT,
    br_win: HBRUSH,
    br_sect: HBRUSH,
    br_input: HBRUSH,
    focused_edit: u16,
    /// Password field: is it currently showing plain text?
    pw_visible: bool,
    /// Divider y-positions for WM_PAINT
    dividers: Vec<i32>,
    /// Inline status position (for WM_PAINT dot)
    status_rect: RECT,
}

// ── Entry point ───────────────────────────────────────────────────────────────
pub fn run(hinstance: HINSTANCE) {
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
            hbrBackground: HBRUSH(0isize),
            lpszClassName: CLASS_NAME,
            hIcon: LoadIconW(hinstance, w!("APP_ICON_IDLE"))
                .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            w!("Backup Sync Tool"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_W,
            100,
            None,
            None,
            hinstance,
            None,
        );
        ShowWindow(hwnd, SW_SHOW);
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
            let hdc = HDC(wparam.0 as isize);
            let hctl = HWND(lparam.0 as isize);
            let id = GetDlgCtrlID(hctl) as u16;
            SetBkMode(hdc, TRANSPARENT);
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            if id == IDC_STATUS_TEXT {
                let clr = if (*st).connected { C_GREEN } else { C_RED };
                SetTextColor(hdc, COLORREF(clr));
                return LRESULT((*st).br_win.0 as isize);
            }
            let text_clr = match id {
                IDC_VERSION => C_HDR,
                IDC_SUBTITLE => C_SUBTITLE,
                IDC_REMOTE_LABEL => C_LABEL,
                _ => C_LABEL,
            };
            SetTextColor(hdc, COLORREF(text_clr));
            LRESULT((*st).br_win.0 as isize)
        }

        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as isize);
            SetBkColor(hdc, COLORREF(C_INPUT_BG));
            SetTextColor(hdc, COLORREF(C_LABEL));
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize);
            }
            LRESULT((*st).br_input.0 as isize)
        }

        WM_CTLCOLORBTN => {
            let hdc = HDC(wparam.0 as isize);
            SetBkMode(hdc, TRANSPARENT);
            let st = state_ptr(hwnd);
            if st.is_null() {
                return LRESULT(GetStockObject(NULL_BRUSH).0 as isize);
            }
            LRESULT((*st).br_win.0 as isize)
        }

        WM_COMMAND => on_command(hwnd, wparam),
        WM_DRAWITEM => on_draw_item(lparam),

        tray::WM_TRAY => on_tray(hwnd, lparam),
        WM_APP_LOG => on_app_log(hwnd, lparam),
        WM_APP_CONNECTED => on_app_connected(hwnd, wparam),
        WM_APP_UPDATE => on_app_update(hwnd, wparam, lparam),

        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState;
            if !st.is_null() {
                tray::remove_tray_icon(hwnd);
                DeleteObject((*st).br_win);
                DeleteObject((*st).br_sect);
                DeleteObject((*st).br_input);
                drop(Box::from_raw(st));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

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

// ── on_create ─────────────────────────────────────────────────────────────────
unsafe fn on_create(hwnd: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as isize);

    let hfont = mkfont("Segoe UI", 12, FW_NORMAL.0 as i32);
    let hfont_hdr = mkfont("Segoe UI", 10, FW_SEMIBOLD.0 as i32);
    let hfont_b = mkfont("Segoe UI", 12, FW_SEMIBOLD.0 as i32);
    let hfont_small = mkfont("Segoe UI", 9, FW_NORMAL.0 as i32);

    let mut cfg = crate::config::load();
    if cfg.watch_folder.is_empty() {
        if let Some(path) = crate::xd::default_watch_folder() {
            cfg.watch_folder = path;
        }
    }
    if cfg.remote_folder.is_empty() {
        if let Some(remote_folder) = crate::xd::detect_default_remote_folder() {
            cfg.remote_folder = remote_folder;
        }
    }
    let pass = secret::decrypt(&cfg.password_enc).unwrap_or_default();

    let state = Box::new(WndState {
        config: cfg.clone(),
        password_plain: pass.clone(),
        sync_engine: None,
        update_url: None,
        connected: false,
        creds_dirty: false,
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        br_win: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_sect: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_input: CreateSolidBrush(COLORREF(C_INPUT_BG)),
        focused_edit: 0,
        pw_visible: false,
        dividers: Vec::new(),
        status_rect: RECT::default(),
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    build_ui(
        hwnd,
        hi,
        &cfg,
        &pass,
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
    );

    let hicon = LoadIconW(hi, w!("APP_ICON_IDLE"))
        .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(hicon.0));
    SendMessageW(
        hwnd,
        WM_SETICON,
        WPARAM(ICON_SMALL as usize),
        LPARAM(hicon.0),
    );
    tray::add_tray_icon(hwnd, hicon);

    let raw = hwnd.0 as isize;

    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        let cfg2 = cfg.clone();
        let pass2 = pass.clone();
        std::thread::spawn(move || {
            let ok = crate::webdav::test_connection(&cfg2, &pass2).is_ok();
            PostMessageW(
                HWND(raw),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        });
    }

    std::thread::spawn(move || {
        if let Some(info) = crate::updater::check(env!("CARGO_PKG_VERSION")) {
            let url = Box::new(info.url);
            PostMessageW(
                HWND(raw),
                WM_APP_UPDATE,
                WPARAM(0),
                LPARAM(Box::into_raw(url) as isize),
            )
            .ok();
        }
    });
}

// ── build_ui ──────────────────────────────────────────────────────────────────
unsafe fn build_ui(
    hwnd: HWND,
    hi: HINSTANCE,
    cfg: &Config,
    pass: &str,
    hf: HFONT,
    hf_hdr: HFONT,
    hf_b: HFONT,
    hf_small: HFONT,
) {
    let st = &mut *state_ptr(hwnd);
    let mut y = M;

    // ── Title + subtitle ──────────────────────────────────────────────────────
    {
        // App title in window caption; add subtitle below
        mkstatic(
            hwnd,
            hi,
            IDC_SUBTITLE,
            "Sync local backups to remote storage",
            M,
            y,
            INNER_W,
            LBL_H,
            hf_small,
        );
        y += LBL_H + SECT;
    }

    // ── SERVER ────────────────────────────────────────────────────────────────
    {
        mklabel_hdr(hwnd, hi, "SERVER", M, y, INNER_W - 90, hf_hdr);

        let conn_w = 80i32;
        let conn_x = M + INNER_W - conn_w;
        mkbtn_blue(
            hwnd,
            hi,
            IDC_CONNECT,
            "Connect",
            conn_x,
            y - 1,
            conn_w,
            CONN_H,
            hf_b,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);

        y += HDR_H + PAD;

        mkfield_label(hwnd, hi, "Server URL", M, y, INNER_W, hf_small);
        y += LBL_H + 4;

        let status_w = 130i32;
        let url_w = INNER_W - status_w - PAD;
        mkedit_cue(
            hwnd,
            hi,
            IDC_URL,
            &cfg.webdav_url,
            "Server URL",
            M,
            y,
            url_w,
            hf,
        );

        // Status text sits to the right of URL input, vertically centred
        let sx = M + url_w + PAD;
        let sy = y + (INP_H - LBL_H) / 2;
        mkstatic(
            hwnd,
            hi,
            IDC_STATUS_TEXT,
            "\u{25cf}  NOT CONNECTED",
            sx,
            sy,
            status_w,
            LBL_H,
            hf,
        );
        // Store the status rect for WM_PAINT dot redraws
        st.status_rect = RECT {
            left: sx,
            top: sy,
            right: sx + status_w,
            bottom: sy + LBL_H,
        };
        y += INP_H + GAP;

        let cred_w = (INNER_W - PAD) / 2;
        mkfield_label(hwnd, hi, "Username", M, y, cred_w, hf_small);
        mkfield_label(hwnd, hi, "Password", M + cred_w + PAD, y, cred_w, hf_small);
        y += LBL_H + 4;

        mkedit_cue(
            hwnd,
            hi,
            IDC_USERNAME,
            &cfg.username,
            "Username",
            M,
            y,
            cred_w,
            hf,
        );
        mkedit_pw_eye(
            hwnd,
            hi,
            IDC_PASSWORD,
            pass,
            M + cred_w + PAD,
            y,
            cred_w,
            hf,
        );
        y += INP_H + SECT;

        st.dividers.push(y - SECT / 2);
    }

    // ── ORIGIN / DESTINATION ──────────────────────────────────────────────────
    {
        mklabel_hdr(hwnd, hi, "ORIGIN / DESTINATION", M, y, INNER_W, hf_hdr);
        y += HDR_H + PAD;

        let browse_x = M + INNER_W - BROWSE_W;
        let inp_w = INNER_W - BROWSE_W - PAD;

        mkfield_label(hwnd, hi, "Origin folder", M, y, INNER_W, hf_small);
        y += LBL_H + 4;
        mkedit_cue(
            hwnd,
            hi,
            IDC_WATCH_FOLDER,
            &cfg.watch_folder,
            "C:\\XDSoftware\\backups",
            M,
            y,
            inp_w,
            hf,
        );
        mkbtn_grey(
            hwnd,
            hi,
            IDC_BROWSE_LOCAL,
            "Browse...",
            browse_x,
            y,
            BROWSE_W,
            INP_H,
            hf,
        );
        y += INP_H + GAP;

        mkfield_label(hwnd, hi, "Destination folder", M, y, INNER_W, hf_small);
        y += LBL_H + 4;
        mkstatic_id(
            hwnd,
            hi,
            IDC_REMOTE_LABEL,
            "Destination folder",
            M,
            y - (LBL_H + 4),
            0,
            LBL_H,
            hf_small,
        );
        mkedit_cue(
            hwnd,
            hi,
            IDC_REMOTE_FOLDER,
            &cfg.remote_folder,
            "XDPT.59655-Palmeira-Minimercado",
            M,
            y,
            inp_w,
            hf,
        );
        mkbtn_grey(
            hwnd,
            hi,
            IDC_BROWSE_REMOTE,
            "Browse...",
            browse_x,
            y,
            BROWSE_W,
            INP_H,
            hf,
        );
        y += INP_H + SECT;

        st.dividers.push(y - SECT / 2);
    }

    // ── RECENT ACTIVITY ───────────────────────────────────────────────────────
    {
        mklabel_hdr(hwnd, hi, "RECENT ACTIVITY", M, y, INNER_W, hf_hdr);
        y += HDR_H + PAD;

        // Reduced height — 72px (3 rows approx) instead of 120px
        let lb_h = 72i32;
        mklb(hwnd, hi, IDC_ACTIVITY_LIST, M, y, INNER_W, lb_h, hf);
        y += lb_h + SECT;
    }

    // ── BOTTOM BAR ────────────────────────────────────────────────────────────
    {
        let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));
        mkstatic(
            hwnd,
            hi,
            IDC_VERSION,
            ver_label,
            M,
            y + (BTN_H - LBL_H) / 2,
            34,
            LBL_H,
            hf_small,
        );
        mkbtn_grey(hwnd, hi, IDC_REPO, "🌍", M + 38, y, 28, BTN_H, hf);

        let opt_y = y + (BTN_H - 20) / 2 - 1;
        mkcheck(
            hwnd,
            hi,
            IDC_START_WINDOWS,
            "Start with Windows",
            M + 72,
            opt_y,
            116,
            20,
            hf_small,
            cfg.start_with_windows,
        );
        mkcheck(
            hwnd,
            hi,
            IDC_SYNC_REMOTE,
            "Sync remote",
            M + 192,
            opt_y,
            90,
            20,
            hf_small,
            cfg.sync_remote_changes,
        );

        let bx_update = M + INNER_W - 80;
        let update_y = y;
        mkbtn_grey(hwnd, hi, IDC_UPDATE, "UPDATE", bx_update, update_y, 80, BTN_H, hf);
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_HIDE);
        y += BTN_H + PAD;

        let close_w = 72i32;
        let save_w = 90i32;
        let bx_close = M + INNER_W - close_w;
        let bx_save = bx_close - GAP - save_w;

        mkbtn_grey(
            hwnd, hi, IDC_CLOSE, "Close", bx_close, y, close_w, BTN_H, hf,
        );
        mkbtn_blue(hwnd, hi, IDC_SAVE, "Save", bx_save, y, save_w, BTN_H, hf_b);

        y += BTN_H + M;
    }

    // Size window to fit content
    let mut wr = RECT::default();
    GetWindowRect(hwnd, &mut wr).ok();
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let dh = (wr.bottom - wr.top) - (cr.bottom - cr.top);
    let dw = (wr.right - wr.left) - (cr.right - cr.left);
    SetWindowPos(
        hwnd,
        None,
        0,
        0,
        WIN_W + dw,
        y + dh,
        SWP_NOMOVE | SWP_NOZORDER,
    )
    .ok();
}

// ── Control helpers ───────────────────────────────────────────────────────────

unsafe fn mklabel_hdr(hwnd: HWND, hi: HINSTANCE, text: &str, x: i32, y: i32, w: i32, hf: HFONT) {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x,
        y,
        w,
        HDR_H,
        hwnd,
        HMENU(0isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
}

unsafe fn mkfield_label(hwnd: HWND, hi: HINSTANCE, text: &str, x: i32, y: i32, w: i32, hf: HFONT) {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x,
        y,
        w,
        LBL_H,
        hwnd,
        HMENU(0isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
}

unsafe fn mkstatic(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

/// Static with no numeric ID (used for pure-label statics that don't need updating)
unsafe fn mkstatic_id(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    mkstatic(hwnd, hi, id, text, x, y, w, h, hf)
}

/// Edit control with a cue banner placeholder (no label needed)
unsafe fn mkedit_cue(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    placeholder: &str,
    x: i32,
    y: i32,
    w: i32,
    hf: HFONT,
) -> HWND {
    let c = mkedit_raw(hwnd, hi, id, text, x, y, w, hf);
    // EM_SETCUEBANNER = 0x1501
    let ph: Vec<u16> = placeholder
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    SendMessageW(c, EM_SETCUEBANNER, WPARAM(1), LPARAM(ph.as_ptr() as isize));
    c
}

unsafe fn mkedit_raw(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("EDIT"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        x,
        y,
        w,
        INP_H,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    let _ = SetWindowSubclass(c, Some(edit_sub), id as usize, 0);
    c
}

/// Password edit with eye icon inside right padding (no separate Show button)
unsafe fn mkedit_pw_eye(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    hf: HFONT,
) -> HWND {
    let c = mkedit_raw(hwnd, hi, id, text, x, y, w, hf);
    // Start hidden
    SendMessageW(c, EM_SETPASSWORDCHAR, WPARAM(0x2022), LPARAM(0));
    // Add right-side padding so text doesn't overlap the eye icon
    // EM_SETMARGINS (0x00D3): HIWORD = right margin, LOWORD = left margin flag
    // EC_RIGHTMARGIN = 0x0002
    let right_margin = (EYE_ZONE_W as u32) << 16;
    SendMessageW(
        c,
        EM_SETMARGINS,
        WPARAM(0x0002),
        LPARAM(right_margin as isize),
    );
    c
}

unsafe fn mkbtn(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}
unsafe fn mkbtn_blue(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    mkbtn(hwnd, hi, id, text, x, y, w, h, hf)
}
unsafe fn mkbtn_grey(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    mkbtn(hwnd, hi, id, text, x, y, w, h, hf)
}

unsafe fn mkcheck(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
    checked: bool,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("BUTTON"),
        &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    SendMessageW(
        c,
        BM_SETCHECK,
        WPARAM(if checked { BST_CHECKED.0 as usize } else { 0 }),
        LPARAM(0),
    );
    c
}

unsafe fn mklb(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
) -> HWND {
    let c = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("LISTBOX"),
        w!(""),
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WINDOW_STYLE(LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_CONNECT, IDC_SAVE];

unsafe fn on_draw_item(lp: LPARAM) -> LRESULT {
    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
    let id = di.CtlID as u16;

    let is_blue = BLUE_IDS.contains(&id);
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

    let hp = CreatePen(PS_SOLID, 1, COLORREF(bc));
    let op = SelectObject(hdc, hp);
    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 5, 5);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(hp);

    let len = GetWindowTextLengthW(di.hwndItem);
    if len > 0 {
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

// ── Commands ──────────────────────────────────────────────────────────────────
unsafe fn on_command(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let id = (wp.0 & 0xFFFF) as u16;
    let notif = (wp.0 >> 16) as u16;

    // EN_CHANGE on credential fields → mark dirty, show Connect button
    if notif == 0x0300u16 && (id == IDC_URL || id == IDC_USERNAME || id == IDC_PASSWORD) {
        let st = stmut(hwnd);
        if !st.creds_dirty {
            st.creds_dirty = true;
            st.connected = false;
            set_status(hwnd, "\u{25cf}  NOT CONNECTED");
            InvalidateRect(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), None, TRUE);
            ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_SHOW);
            EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), TRUE);
        }
        return LRESULT(0);
    }

    match id {
        IDC_BROWSE_LOCAL => browse_local(hwnd),
        IDC_BROWSE_REMOTE => {
            SetFocus(GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32));
        }
        IDC_CONNECT => do_connect(hwnd),
        IDC_SAVE => do_save(hwnd),
        IDC_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
        }
        IDC_REPO => do_open_repo(hwnd),
        IDC_UPDATE => do_update(hwnd),
        _ => {}
    }
    LRESULT(0)
}

unsafe fn browse_local(hwnd: HWND) {
    let title: Vec<u16> = "Select local folder\0".encode_utf16().collect();
    let mut display = [0u16; 260];
    let mut bi = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        pszDisplayName: PWSTR(display.as_mut_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let pidl = SHBrowseForFolderW(&mut bi);
    if pidl.is_null() {
        return;
    }
    let mut buf = [0u16; 260];
    if SHGetPathFromIDListW(pidl, &mut buf).as_bool() {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32), &hstring(&s));
    }
    ILFree(Some(pidl));
}

unsafe fn do_connect(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), FALSE);
    set_status(hwnd, "\u{25cf}  Connecting\u{2026}");
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let ok = webdav::test_connection(&cfg, &pass).is_ok();
        PostMessageW(
            HWND(raw),
            WM_APP_CONNECTED,
            WPARAM(if ok { 1 } else { 0 }),
            LPARAM(0),
        )
        .ok();
    });
}

unsafe fn do_save(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    match secret::encrypt(&st.password_plain) {
        Ok(enc) => st.config.password_enc = enc,
        Err(e) => {
            msgbox(hwnd, &format!("Encrypt error: {e}"), "Error");
            return;
        }
    }
    if let Err(e) = crate::config::save(&st.config) {
        msgbox(hwnd, &format!("Save error: {e}"), "Error");
        return;
    }
    apply_startup(&st.config);
    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    let raw = hwnd.0 as isize;
    let log: crate::sync::LogFn = Arc::new(move |m: String| {
        let s = Box::new(m);
        unsafe {
            PostMessageW(
                HWND(raw),
                WM_APP_LOG,
                WPARAM(0),
                LPARAM(Box::into_raw(s) as isize),
            )
            .ok();
        }
    });
    match crate::sync::SyncEngine::start(cfg.clone(), pass.clone(), log) {
        Ok(e) => stmut(hwnd).sync_engine = Some(e),
        Err(e) => {
            msgbox(hwnd, &format!("Sync error: {e}"), "Error");
        }
    }
    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        set_status(hwnd, "\u{25cf}  Connecting\u{2026}");
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
        std::thread::spawn(move || {
            let ok = webdav::test_connection(&cfg, &pass).is_ok();
            PostMessageW(
                HWND(raw),
                WM_APP_CONNECTED,
                WPARAM(if ok { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        });
    }
}

unsafe fn do_update(hwnd: HWND) {
    let url = match stmut(hwnd).update_url.clone() {
        Some(u) => u,
        None => return,
    };
    if msgbox_yn(
        hwnd,
        "A new version is available.\nDownload and install now? The app will restart.",
        "Update Available",
    ) {
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_HIDE);
        let raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = crate::updater::download_and_replace(&url, |pct| {
                let m = Box::new(format!("Downloading: {pct}%"));
                unsafe {
                    PostMessageW(
                        HWND(raw),
                        WM_APP_LOG,
                        WPARAM(0),
                        LPARAM(Box::into_raw(m) as isize),
                    )
                    .ok();
                }
            });
        });
    }
}

unsafe fn do_open_repo(hwnd: HWND) {
    let _ = ShellExecuteW(
        hwnd,
        w!("open"),
        &hstring(REPO_URL),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    let hlb = GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32);
    let ws = hstring(&format!("{}  {}", ts(), msg));
    SendMessageW(
        hlb,
        LB_INSERTSTRING,
        WPARAM(0),
        LPARAM(ws.as_ptr() as isize),
    );
    if SendMessageW(hlb, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0 > 200 {
        SendMessageW(hlb, LB_DELETESTRING, WPARAM(200), LPARAM(0));
    }
    LRESULT(0)
}

unsafe fn on_app_connected(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let connected = wp.0 == 1;
    let st = stmut(hwnd);
    st.connected = connected;
    if connected {
        set_status(hwnd, "\u{25cf}  Connected");
        st.creds_dirty = false;
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
    } else {
        set_status(hwnd, "\u{25cf}  Not connected");
        EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), TRUE);
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_SHOW);
    }
    InvalidateRect(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), None, TRUE);
    LRESULT(0)
}

unsafe fn on_app_update(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 == 1 {
        return LRESULT(0);
    }
    let url = Box::from_raw(lp.0 as *mut String);
    stmut(hwnd).update_url = Some(*url);
    ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_UPDATE as i32), None, TRUE);
    let ver_update = concat!("v", env!("CARGO_PKG_VERSION"), " \u{2191}");
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_VERSION as i32), &hstring(ver_update));
    LRESULT(0)
}

unsafe fn on_tray(hwnd: HWND, lp: LPARAM) -> LRESULT {
    match (lp.0 & 0xFFFF) as u32 {
        WM_LBUTTONDBLCLK => {
            ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        WM_RBUTTONUP => tray::show_tray_menu(hwnd),
        _ => {}
    }
    LRESULT(0)
}

// ── Utilities ─────────────────────────────────────────────────────────────────
unsafe fn set_status(hwnd: HWND, t: &str) {
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), &hstring(t));
}

unsafe fn read_ctrls(hwnd: HWND, st: &mut WndState) {
    st.config.watch_folder = gettext(hwnd, IDC_WATCH_FOLDER);
    st.config.webdav_url = gettext(hwnd, IDC_URL);
    st.config.username = gettext(hwnd, IDC_USERNAME);
    st.password_plain = gettext(hwnd, IDC_PASSWORD);
    st.config.remote_folder = gettext(hwnd, IDC_REMOTE_FOLDER);
    st.config.start_with_windows = checked(hwnd, IDC_START_WINDOWS);
    st.config.sync_remote_changes = checked(hwnd, IDC_SYNC_REMOTE);
}

unsafe fn gettext(hwnd: HWND, id: u16) -> String {
    let h = GetDlgItem(hwnd, id as i32);
    let n = GetWindowTextLengthW(h);
    if n == 0 {
        return String::new();
    }
    let mut b = vec![0u16; (n + 1) as usize];
    GetWindowTextW(h, &mut b);
    String::from_utf16_lossy(&b[..n as usize])
}

unsafe fn checked(hwnd: HWND, id: u16) -> bool {
    SendMessageW(
        GetDlgItem(hwnd, id as i32),
        BM_GETCHECK,
        WPARAM(0),
        LPARAM(0),
    )
    .0 == BST_CHECKED.0 as isize
}

unsafe fn stmut(hwnd: HWND) -> &'static mut WndState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState)
}
unsafe fn state_ptr(hwnd: HWND) -> *mut WndState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WndState
}

unsafe fn mkfont(name: &str, pt: i32, weight: i32) -> HFONT {
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
    ReleaseDC(None, hdc);
    let h = -(pt * dpi / 72);
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW::default();
    lf.lfHeight = h;
    lf.lfWeight = weight;
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

fn hstring(s: &str) -> HSTRING {
    HSTRING::from(s)
}
#[allow(dead_code)]
fn wstr(b: &[u16]) -> String {
    let e = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..e])
}

unsafe fn msgbox(hwnd: HWND, text: &str, title: &str) {
    MessageBoxW(
        hwnd,
        &hstring(text),
        &hstring(title),
        MB_OK | MB_ICONINFORMATION,
    );
}
unsafe fn msgbox_yn(hwnd: HWND, text: &str, title: &str) -> bool {
    MessageBoxW(
        hwnd,
        &hstring(text),
        &hstring(title),
        MB_YESNO | MB_ICONQUESTION,
    )
    .0 == IDYES.0 as i32
}

fn ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

unsafe fn apply_startup(cfg: &Config) {
    use windows::Win32::System::Registry::*;
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut hk = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, key, 0, KEY_SET_VALUE, &mut hk).is_ok() {
        if cfg.start_with_windows {
            if let Ok(exe) = std::env::current_exe() {
                let v: Vec<u16> = exe
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let _ = RegSetValueExW(
                    hk,
                    w!("BackupSyncTool"),
                    0,
                    REG_SZ,
                    Some(bytemuck::cast_slice(&v)),
                );
            }
        } else {
            let _ = RegDeleteValueW(hk, w!("BackupSyncTool"));
        }
        let _ = RegCloseKey(hk);
    }
}
