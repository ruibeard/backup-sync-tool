// ui.rs — Win32 main window
//
// Design:
//   Window bg:       #F0F0F0
//   Section cards:   #F8F8F8 bg, 1px #DEDEDE border, r=4, 6px gap
//   Row labels:      #333333, Segoe UI 12pt
//   Section headers: #888888, Segoe UI 10pt SemiBold, ALL CAPS
//   Inputs:          white bg, 1px #CCCCCC border (blue #2B4FA3 on focus)
//   Connect/Save:    blue #2B4FA3, white text
//   Browse/Close:    #E8E8E8 grey, #333333 text
//   Bottom bar:      version left, CHECK / SAVE / CLOSE right
//
// Sections are NOT collapsible — static layout, no toggle logic.
// All controls are direct children of hwnd so WM_DRAWITEM/WM_CTLCOLOR*
// arrive at the main wnd proc without re-routing.

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
    DefSubclassProc, IFileOpenDialog, FileOpenDialog,
    SetWindowSubclass, FOS_PICKFOLDERS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── Colours  0x00BBGGRR ──────────────────────────────────────────────────────
const C_WIN_BG:      u32 = 0x00F0F0F0;
const C_SECT_BG:     u32 = 0x00F8F8F8;
const C_SECT_BORDER: u32 = 0x00DEDEDE;
const C_LABEL:       u32 = 0x00333333;
const C_HDR:         u32 = 0x00888888;
const C_INPUT_BG:    u32 = 0x00FFFFFF;
const C_INPUT_BORDER:u32 = 0x00CCCCCC;
const C_INPUT_FOCUS: u32 = 0x00A34F2B; // #2B4FA3 BGR
const C_BLUE:        u32 = 0x00A34F2B;
const C_BLUE_HOV:    u32 = 0x007A3A1E;
const C_BLUE_TXT:    u32 = 0x00FFFFFF;
const C_GREY_BTN:    u32 = 0x00E8E8E8;
const C_GREY_HOV:    u32 = 0x00D8D8D8;
const C_GREY_TXT:    u32 = 0x00333333;
const C_GREY_BORDER: u32 = 0x00BBBBBB;

// ── Control IDs ──────────────────────────────────────────────────────────────
const IDC_WATCH_FOLDER:  u16 = 101;
const IDC_BROWSE_LOCAL:  u16 = 102;
const IDC_URL:           u16 = 103;
const IDC_USERNAME:      u16 = 104;
const IDC_PASSWORD:      u16 = 105;
const IDC_REMOTE_FOLDER: u16 = 106;
const IDC_BROWSE_REMOTE: u16 = 107;
const IDC_CONNECT:       u16 = 108;
const IDC_STATUS_TEXT:   u16 = 109;
const IDC_SAVE:          u16 = 110;
const IDC_CLOSE:         u16 = 111;
const IDC_UPDATE:        u16 = 112;
const IDC_VERSION:       u16 = 113;
const IDC_ACTIVITY_LIST: u16 = 114;
const IDC_START_WINDOWS: u16 = 115;
const IDC_SYNC_REMOTE:   u16 = 116;
const IDC_SHOW_PASSWORD: u16 = 117;

const WM_APP_LOG:       u32 = WM_APP + 10;
const WM_APP_CONNECTED: u32 = WM_APP + 11;
const WM_APP_UPDATE:    u32 = WM_APP + 12;

const SS_LEFT: u32 = 0x0000;

pub const CLASS_NAME: PCWSTR = w!("BackupSyncToolWnd");

// ── Layout ───────────────────────────────────────────────────────────────────
const WIN_W:    i32 = 480;  // client width
const M:        i32 = 10;   // outer margin
const PAD:      i32 = 12;   // inner card padding
const LBL_W:    i32 = 100;  // label column width
const INP_H:    i32 = 24;   // input / browse height
const BTN_H:    i32 = 28;   // bottom-bar button height
const CONN_H:   i32 = 28;   // Connect button height
const ROW_GAP:  i32 = 6;    // vertical gap between rows
const HDR_H:    i32 = 32;   // card header height
const CARD_GAP: i32 = 6;    // gap between cards
const BROWSE_W: i32 = 72;   // Browse button width
const LBL_H:    i32 = 18;   // label text height (12pt)

// ── Card rect list (for WM_PAINT) ─────────────────────────────────────────────
#[derive(Clone, Copy)]
struct CardRect { left: i32, top: i32, right: i32, bottom: i32 }

// ── Window state ──────────────────────────────────────────────────────────────
struct WndState {
    config:         Config,
    password_plain: String,
    sync_engine:    Option<crate::sync::SyncEngine>,
    update_url:     Option<String>,
    #[allow(dead_code)] hfont:     HFONT,
    #[allow(dead_code)] hfont_hdr: HFONT,
    #[allow(dead_code)] hfont_b:   HFONT,
    // Cached brushes — returned from WM_CTLCOLOR*, never re-created per message
    br_win:   HBRUSH,
    br_sect:  HBRUSH,
    br_input: HBRUSH,
    cards:    Vec<CardRect>,
    focused_edit: u16,
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
            cbSize:        std::mem::size_of::<WNDCLASSEXW>() as u32,
            style:         CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc:   Some(wnd_proc),
            hInstance:     hinstance,
            hCursor:       LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(0isize), // we paint manually
            lpszClassName: CLASS_NAME,
            hIcon: LoadIconW(hinstance, w!("APP_ICON_IDLE"))
                .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(), CLASS_NAME, w!("Backup Sync Tool"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT, CW_USEDEFAULT, WIN_W, 100,
            None, None, hinstance, None,
        );
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 || ret.0 == -1 { break; }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ── Window procedure ──────────────────────────────────────────────────────────
unsafe extern "system" fn wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE    => { on_create(hwnd); LRESULT(0) }
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
            let hdc  = HDC(wparam.0 as isize);
            let hctl = HWND(lparam.0 as isize);
            let id   = GetDlgCtrlID(hctl) as u16;
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(if id == IDC_VERSION { C_HDR } else { C_LABEL }));
            let st = state_ptr(hwnd);
            if st.is_null() { return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize); }
            // VERSION sits on window bg; card controls sit on card bg
            let br = if id == IDC_VERSION { (*st).br_win } else { (*st).br_sect };
            LRESULT(br.0 as isize)
        }

        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as isize);
            SetBkColor(hdc, COLORREF(C_INPUT_BG));
            SetTextColor(hdc, COLORREF(C_LABEL));
            let st = state_ptr(hwnd);
            if st.is_null() { return LRESULT(GetStockObject(WHITE_BRUSH).0 as isize); }
            LRESULT((*st).br_input.0 as isize)
        }

        WM_CTLCOLORBTN => {
            let hdc = HDC(wparam.0 as isize);
            SetBkMode(hdc, TRANSPARENT);
            let st = state_ptr(hwnd);
            if st.is_null() { return LRESULT(GetStockObject(NULL_BRUSH).0 as isize); }
            LRESULT((*st).br_sect.0 as isize)
        }

        WM_COMMAND  => on_command(hwnd, wparam),
        WM_DRAWITEM => on_draw_item(lparam),

        tray::WM_TRAY    => on_tray(hwnd, lparam),
        WM_APP_LOG       => on_app_log(hwnd, lparam),
        WM_APP_CONNECTED => on_app_connected(hwnd, wparam),
        WM_APP_UPDATE    => on_app_update(hwnd, wparam, lparam),

        WM_CLOSE   => { ShowWindow(hwnd, SW_HIDE); LRESULT(0) }
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
unsafe fn paint_bg(hwnd: HWND, hdc: HDC) {
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();

    // Window fill
    let br = CreateSolidBrush(COLORREF(C_WIN_BG));
    FillRect(hdc, &cr, br);
    DeleteObject(br);

    // Card backgrounds — no border, just fill
    let st = state_ptr(hwnd);
    if st.is_null() { return; }
    for c in &(*st).cards {
        let rc = RECT { left: c.left, top: c.top, right: c.right, bottom: c.bottom };
        let br2 = CreateSolidBrush(COLORREF(C_SECT_BG));
        FillRect(hdc, &rc, br2);
        DeleteObject(br2);
    }
}

// ── Edit subclass: flat 1px border ────────────────────────────────────────────
unsafe extern "system" fn edit_sub(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _uid: usize, _ref: usize,
) -> LRESULT {
    match msg {
        WM_SETFOCUS | WM_KILLFOCUS => {
            let id = GetDlgCtrlID(hwnd) as u16;
            let st = state_ptr(GetParent(hwnd));
            if !st.is_null() {
                (*st).focused_edit = if msg == WM_SETFOCUS { id } else { 0 };
            }
            let r = DefSubclassProc(hwnd, msg, wp, lp);
            SetWindowPos(hwnd, None, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED).ok();
            r
        }
        WM_NCPAINT => {
            let id  = GetDlgCtrlID(hwnd) as u16;
            let st  = state_ptr(GetParent(hwnd));
            let focused = !st.is_null() && (*st).focused_edit == id;
            let hdc = GetWindowDC(hwnd);
            let mut wr = RECT::default();
            GetWindowRect(hwnd, &mut wr).ok();
            let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
            let clr  = if focused { C_INPUT_FOCUS } else { C_INPUT_BORDER };
            let hpen = CreatePen(PS_SOLID, 1, COLORREF(clr));
            let op   = SelectObject(hdc, hpen);
            let ob   = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            Rectangle(hdc, 0, 0, w, h);
            SelectObject(hdc, op); SelectObject(hdc, ob);
            DeleteObject(hpen);
            ReleaseDC(hwnd, hdc);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

// ── on_create ─────────────────────────────────────────────────────────────────
unsafe fn on_create(hwnd: HWND) {
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as isize);

    // Fonts: normal 12pt, header 10pt SemiBold, bold 12pt SemiBold
    let hfont     = mkfont("Segoe UI", 12, FW_NORMAL.0 as i32);
    let hfont_hdr = mkfont("Segoe UI", 10, FW_SEMIBOLD.0 as i32);
    let hfont_b   = mkfont("Segoe UI", 12, FW_SEMIBOLD.0 as i32);

    let mut cfg = crate::config::load();
    // Pre-fill default local folder if never set
    if cfg.watch_folder.is_empty() {
        cfg.watch_folder = r"C:\XDSoftware\backups".to_string();
    }
    let pass  = secret::decrypt(&cfg.password_enc).unwrap_or_default();

    let state = Box::new(WndState {
        config: cfg.clone(), password_plain: pass.clone(),
        sync_engine: None, update_url: None,
        hfont, hfont_hdr, hfont_b,
        br_win:   CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_sect:  CreateSolidBrush(COLORREF(C_SECT_BG)),
        br_input: CreateSolidBrush(COLORREF(C_INPUT_BG)),
        cards: Vec::new(),
        focused_edit: 0,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

    build_ui(hwnd, hi, &cfg, &pass, hfont, hfont_hdr, hfont_b);

    let hicon = LoadIconW(hi, w!("APP_ICON_IDLE"))
        .unwrap_or(LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize),   LPARAM(hicon.0));
    SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(hicon.0));
    tray::add_tray_icon(hwnd, hicon);

    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        if let Some(info) = crate::updater::check(env!("CARGO_PKG_VERSION")) {
            let url = Box::new(info.url);
            PostMessageW(HWND(raw), WM_APP_UPDATE, WPARAM(0),
                LPARAM(Box::into_raw(url) as isize)).ok();
        }
    });
}

// ── build_ui ──────────────────────────────────────────────────────────────────
unsafe fn build_ui(
    hwnd: HWND, hi: HINSTANCE,
    cfg: &Config, pass: &str,
    hf: HFONT, hf_hdr: HFONT, hf_b: HFONT,
) {
    let iw  = WIN_W - M * 2; // inner width
    let ix  = M + PAD + LBL_W + 6; // input x
    let mut y = M;

    let st = &mut *state_ptr(hwnd);

    // ── LOCAL FOLDER ──────────────────────────────────────────────────────────
    {
        let card_y = y;
        let inp_w  = WIN_W - M - PAD - ix - 6 - BROWSE_W;

        // Header
        mklabel_hdr(hwnd, hi, "LOCAL FOLDER", M + PAD, y + (HDR_H - LBL_H) / 2, iw - PAD * 2, hf_hdr);
        y += HDR_H;

        // Separator line
        mksep(hwnd, hi, M, y, WIN_W - M);
        y += 1;

        // Content
        y += PAD;
        mklabel(hwnd, hi, "Folder", M + PAD, y, LBL_W, hf);
        mkedit(hwnd, hi, IDC_WATCH_FOLDER, &cfg.watch_folder, ix, y, inp_w, hf);
        mkbtn_grey(hwnd, hi, IDC_BROWSE_LOCAL, "Browse...", ix + inp_w + 6, y, BROWSE_W, INP_H, hf);
        y += INP_H + PAD;

        st.cards.push(CardRect { left: M, top: card_y, right: M + iw, bottom: y });
        y += CARD_GAP;
    }

    // ── SERVER ────────────────────────────────────────────────────────────────
    {
        let card_y = y;
        let inp_w  = WIN_W - M - PAD - ix;
        let rf_w   = inp_w - 6 - BROWSE_W;

        mklabel_hdr(hwnd, hi, "SERVER", M + PAD, y + (HDR_H - LBL_H) / 2, iw - PAD * 2, hf_hdr);
        y += HDR_H;
        mksep(hwnd, hi, M, y, WIN_W - M);
        y += 1 + PAD;

        mklabel(hwnd, hi, "URL",           M + PAD, y, LBL_W, hf);
        mkedit(hwnd, hi, IDC_URL,       &cfg.webdav_url, ix, y, inp_w, hf);
        y += INP_H + ROW_GAP;

        mklabel(hwnd, hi, "Username",      M + PAD, y, LBL_W, hf);
        mkedit(hwnd, hi, IDC_USERNAME,  &cfg.username,   ix, y, inp_w, hf);
        y += INP_H + ROW_GAP;

        mklabel(hwnd, hi, "Password",      M + PAD, y, LBL_W, hf);
        mkedit_pw(hwnd, hi, IDC_PASSWORD, pass, ix, y, inp_w - 6 - BROWSE_W, hf);
        mkbtn_grey(hwnd, hi, IDC_SHOW_PASSWORD, "Show", ix + inp_w - BROWSE_W, y, BROWSE_W, INP_H, hf);
        y += INP_H + ROW_GAP;

        mklabel(hwnd, hi, "Remote folder", M + PAD, y, LBL_W, hf);
        mkedit(hwnd, hi, IDC_REMOTE_FOLDER, &cfg.remote_folder, ix, y, rf_w, hf);
        mkbtn_grey(hwnd, hi, IDC_BROWSE_REMOTE, "Browse...", ix + rf_w + 6, y, BROWSE_W, INP_H, hf);
        y += INP_H + ROW_GAP;

        // Connect row
        mkbtn_blue(hwnd, hi, IDC_CONNECT, "Connect", M + PAD, y, 110, CONN_H, hf_b);
        mkstatic(hwnd, hi, IDC_STATUS_TEXT, "\u{25cf}  NOT CONNECTED",
            M + PAD + 110 + 12, y + (CONN_H - LBL_H) / 2, 200, LBL_H, hf);
        y += CONN_H + PAD;

        st.cards.push(CardRect { left: M, top: card_y, right: M + iw, bottom: y });
        y += CARD_GAP;
    }

    // ── OPTIONS ───────────────────────────────────────────────────────────────
    {
        let card_y = y;

        mklabel_hdr(hwnd, hi, "OPTIONS", M + PAD, y + (HDR_H - LBL_H) / 2, iw - PAD * 2, hf_hdr);
        y += HDR_H;
        mksep(hwnd, hi, M, y, WIN_W - M);
        y += 1 + PAD;

        mkcheck(hwnd, hi, IDC_START_WINDOWS, "Start with Windows",
            M + PAD, y, 240, 20, hf, cfg.start_with_windows);
        y += 20 + ROW_GAP;
        mkcheck(hwnd, hi, IDC_SYNC_REMOTE,   "Sync remote changes",
            M + PAD, y, 240, 20, hf, cfg.sync_remote_changes);
        y += 20 + PAD;

        st.cards.push(CardRect { left: M, top: card_y, right: M + iw, bottom: y });
        y += CARD_GAP;
    }

    // ── RECENT ACTIVITY ───────────────────────────────────────────────────────
    {
        let card_y = y;
        let lb_h   = 120i32;

        mklabel_hdr(hwnd, hi, "RECENT ACTIVITY", M + PAD, y + (HDR_H - LBL_H) / 2, iw - PAD * 2, hf_hdr);
        y += HDR_H;
        mksep(hwnd, hi, M, y, WIN_W - M);
        y += 1 + PAD;

        mklb(hwnd, hi, IDC_ACTIVITY_LIST, M + PAD, y, iw - PAD * 2, lb_h, hf);
        y += lb_h + PAD;

        st.cards.push(CardRect { left: M, top: card_y, right: M + iw, bottom: y });
        y += CARD_GAP;
    }

    // ── BOTTOM BAR ────────────────────────────────────────────────────────────
    {
        let bar_y  = y;
        let bar_h  = BTN_H + 10 * 2;
        let by     = bar_y + (bar_h - BTN_H) / 2;

        let ver_label = concat!("BACKUP SYNC TOOL V", env!("CARGO_PKG_VERSION"));
        mkstatic(hwnd, hi, IDC_VERSION, ver_label,
            M, by + (BTN_H - LBL_H) / 2, 200, LBL_H, hf_hdr);

        let bx_close  = WIN_W - M - 80;
        let bx_save   = bx_close  - 6 - 90;
        let bx_update = bx_save   - 6 - 90;

        mkbtn_grey(hwnd, hi, IDC_CLOSE,  "CLOSE",  bx_close,  by, 80, BTN_H, hf);
        mkbtn_blue(hwnd, hi, IDC_SAVE,   "SAVE",   bx_save,   by, 90, BTN_H, hf_b);
        // UPDATE button — hidden until background check finds a newer version
        mkbtn_grey(hwnd, hi, IDC_UPDATE, "UPDATE", bx_update, by, 90, BTN_H, hf);
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_HIDE);

        y += bar_h;
    }

    // Size window to fit
    let mut wr = RECT::default();
    GetWindowRect(hwnd, &mut wr).ok();
    let mut cr = RECT::default();
    GetClientRect(hwnd, &mut cr).ok();
    let dh = (wr.bottom - wr.top) - (cr.bottom - cr.top);
    let dw = (wr.right  - wr.left) - (cr.right  - cr.left);
    SetWindowPos(hwnd, None, 0, 0, WIN_W + dw, y + M + dh, SWP_NOMOVE | SWP_NOZORDER).ok();
}

// ── Control helpers ───────────────────────────────────────────────────────────

unsafe fn mklabel_hdr(hwnd: HWND, hi: HINSTANCE, text: &str, x: i32, y: i32, w: i32, hf: HFONT) {
    let hs = hstring(text);
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x, y, w, LBL_H, hwnd, HMENU(0isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
}

unsafe fn mklabel(hwnd: HWND, hi: HINSTANCE, text: &str, x: i32, y: i32, w: i32, hf: HFONT) {
    let hs = hstring(text);
    // Vertically centre label text against INP_H
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x, y + (INP_H - LBL_H) / 2, w, LBL_H, hwnd, HMENU(0isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
}

unsafe fn mkstatic(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT) -> HWND
{
    let hs = hstring(text);
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x, y, w, h, hwnd, HMENU(id as isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

/// Horizontal separator — subclassed to paint as #DEDEDE
unsafe fn mksep(hwnd: HWND, hi: HINSTANCE, x: i32, y: i32, x2: i32) {
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), w!(""),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        x, y, x2 - x, 1, hwnd, HMENU(0isize), hi, None);
    let _ = SetWindowSubclass(c, Some(sep_proc), 0, 0);
}

unsafe extern "system" fn sep_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _uid: usize, _ref: usize,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc).ok();
            let br = CreateSolidBrush(COLORREF(C_SECT_BORDER));
            FillRect(hdc, &rc, br);
            DeleteObject(br);
            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

unsafe fn mkedit(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, hf: HFONT) -> HWND
{
    let hs = hstring(text);
    let c = CreateWindowExW(WS_EX_CLIENTEDGE, w!("EDIT"), &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        x, y, w, INP_H, hwnd, HMENU(id as isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    let _ = SetWindowSubclass(c, Some(edit_sub), id as usize, 0);
    c
}

unsafe fn mkedit_pw(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, hf: HFONT) -> HWND
{
    let c = mkedit(hwnd, hi, id, text, x, y, w, hf);
    SendMessageW(c, EM_SETPASSWORDCHAR, WPARAM(0x2022), LPARAM(0));
    c
}

unsafe fn mkbtn(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT) -> HWND
{
    let hs = hstring(text);
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("BUTTON"), &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        x, y, w, h, hwnd, HMENU(id as isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}
unsafe fn mkbtn_blue(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT) -> HWND { mkbtn(hwnd,hi,id,text,x,y,w,h,hf) }
unsafe fn mkbtn_grey(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT) -> HWND { mkbtn(hwnd,hi,id,text,x,y,w,h,hf) }

unsafe fn mkcheck(hwnd: HWND, hi: HINSTANCE, id: u16, text: &str,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT, checked: bool) -> HWND
{
    let hs = hstring(text);
    let c = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("BUTTON"), &hs,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        x, y, w, h, hwnd, HMENU(id as isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    SendMessageW(c, BM_SETCHECK,
        WPARAM(if checked { BST_CHECKED.0 as usize } else { 0 }), LPARAM(0));
    c
}

unsafe fn mklb(hwnd: HWND, hi: HINSTANCE, id: u16,
    x: i32, y: i32, w: i32, h: i32, hf: HFONT) -> HWND
{
    let c = CreateWindowExW(WS_EX_CLIENTEDGE, w!("LISTBOX"), w!(""),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL
            | WINDOW_STYLE(LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32),
        x, y, w, h, hwnd, HMENU(id as isize), hi, None);
    SendMessageW(c, WM_SETFONT, WPARAM(hf.0 as usize), LPARAM(1));
    c
}

// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_CONNECT, IDC_SAVE];

unsafe fn on_draw_item(lp: LPARAM) -> LRESULT {
    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
    let id = di.CtlID as u16;

    let is_blue    = BLUE_IDS.contains(&id);
    let pressed    = (di.itemState.0 & ODS_SELECTED.0) != 0;
    let disabled   = (di.itemState.0 & ODS_DISABLED.0) != 0;

    let (bg, fg, bc) = if disabled {
        (C_GREY_BTN, 0x00AAAAAA_u32, C_SECT_BORDER)
    } else if is_blue {
        let b = if pressed { C_BLUE_HOV } else { C_BLUE };
        (b, C_BLUE_TXT, b)
    } else {
        let b = if pressed { C_GREY_HOV } else { C_GREY_BTN };
        (b, C_GREY_TXT, C_GREY_BORDER)
    };

    let rc  = di.rcItem;
    let hdc = di.hDC;

    let hbr = CreateSolidBrush(COLORREF(bg));
    FillRect(hdc, &rc, hbr);
    DeleteObject(hbr);

    let hp  = CreatePen(PS_SOLID, 1, COLORREF(bc));
    let op  = SelectObject(hdc, hp);
    let ob  = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 5, 5);
    SelectObject(hdc, op); SelectObject(hdc, ob);
    DeleteObject(hp);

    let len = GetWindowTextLengthW(di.hwndItem);
    if len > 0 {
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(di.hwndItem, &mut buf);
        let hf  = HFONT(SendMessageW(di.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as isize);
        let of  = SelectObject(hdc, hf);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(fg));
        let mut tr = rc;
        tr.left += 4; tr.right -= 4;
        DrawTextW(hdc, &mut buf[..len as usize], &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(hdc, of);
    }

    if (di.itemState.0 & ODS_FOCUS.0) != 0 {
        let mut fr = rc;
        fr.left += 3; fr.top += 3; fr.right -= 3; fr.bottom -= 3;
        DrawFocusRect(hdc, &fr);
    }
    LRESULT(1)
}

// ── Commands ──────────────────────────────────────────────────────────────────
unsafe fn on_command(hwnd: HWND, wp: WPARAM) -> LRESULT {
    match (wp.0 & 0xFFFF) as u16 {
        IDC_BROWSE_LOCAL  => browse_local(hwnd),
        IDC_BROWSE_REMOTE => { SetFocus(GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32)); }
        IDC_CONNECT       => do_connect(hwnd),
        IDC_SAVE          => do_save(hwnd),
        IDC_CLOSE         => { ShowWindow(hwnd, SW_HIDE); }
        IDC_UPDATE        => do_update(hwnd),
        IDC_SHOW_PASSWORD => toggle_password(hwnd),
        _ => {}
    }
    LRESULT(0)
}

unsafe fn toggle_password(hwnd: HWND) {
    let hedit = GetDlgItem(hwnd, IDC_PASSWORD as i32);
    let hbtn  = GetDlgItem(hwnd, IDC_SHOW_PASSWORD as i32);
    // If password char is currently set, it returns the char; 0 means plain text
    let currently_hidden = SendMessageW(hedit, EM_GETPASSWORDCHAR, WPARAM(0), LPARAM(0)).0 != 0;
    if currently_hidden {
        // Reveal
        SendMessageW(hedit, EM_SETPASSWORDCHAR, WPARAM(0), LPARAM(0));
        let _ = SetWindowTextW(hbtn, w!("Hide"));
    } else {
        // Hide again
        SendMessageW(hedit, EM_SETPASSWORDCHAR, WPARAM(0x2022), LPARAM(0));
        let _ = SetWindowTextW(hbtn, w!("Show"));
    }
    InvalidateRect(hedit, None, TRUE);
}

unsafe fn browse_local(hwnd: HWND) {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{SHCreateItemFromParsingName, IShellItem};
    if let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) {
        let _ = dialog.SetOptions(FOS_PICKFOLDERS);
        // Pre-navigate to the current folder value if it exists
        let current = gettext(hwnd, IDC_WATCH_FOLDER);
        if !current.is_empty() {
            let wide: Vec<u16> = current.encode_utf16().chain(std::iter::once(0)).collect();
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        if dialog.Show(hwnd).is_ok() {
            if let Ok(item) = dialog.GetResult() {
                if let Ok(path) = item.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH) {
                    let s = path.to_string().unwrap_or_default();
                    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_WATCH_FOLDER as i32), &hstring(&s));
                    windows::Win32::System::Com::CoTaskMemFree(Some(path.as_ptr() as _));
                }
            }
        }
    }
}

unsafe fn do_connect(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    let cfg = st.config.clone(); let pass = st.password_plain.clone();
    EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), FALSE);
    set_status(hwnd, "\u{25cf}  Connecting\u{2026}");
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let ok = webdav::test_connection(&cfg, &pass).is_ok();
        PostMessageW(HWND(raw), WM_APP_CONNECTED,
            WPARAM(if ok { 1 } else { 0 }), LPARAM(0)).ok();
    });
}

unsafe fn do_save(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);
    match secret::encrypt(&st.password_plain) {
        Ok(enc) => st.config.password_enc = enc,
        Err(e)  => { msgbox(hwnd, &format!("Encrypt error: {e}"), "Error"); return; }
    }
    if let Err(e) = crate::config::save(&st.config) {
        msgbox(hwnd, &format!("Save error: {e}"), "Error"); return;
    }
    apply_startup(&st.config);
    let cfg = st.config.clone(); let pass = st.password_plain.clone();
    let raw = hwnd.0 as isize;
    let log: crate::sync::LogFn = Arc::new(move |m: String| {
        let s = Box::new(m);
        unsafe { PostMessageW(HWND(raw), WM_APP_LOG, WPARAM(0),
            LPARAM(Box::into_raw(s) as isize)).ok(); }
    });
    match crate::sync::SyncEngine::start(cfg, pass, log) {
        Ok(e)  => st.sync_engine = Some(e),
        Err(e) => { msgbox(hwnd, &format!("Sync error: {e}"), "Error"); }
    }
    msgbox(hwnd, "Configuration saved.", "Backup Sync Tool");
}

unsafe fn do_update(hwnd: HWND) {
    let url = match stmut(hwnd).update_url.clone() { Some(u) => u, None => return };
    if msgbox_yn(hwnd, "A new version is available.\nDownload and install now? The app will restart.", "Update Available") {
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_HIDE);
        let raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = crate::updater::download_and_replace(&url, |pct| {
                let m = Box::new(format!("Downloading: {pct}%"));
                unsafe { PostMessageW(HWND(raw), WM_APP_LOG, WPARAM(0),
                    LPARAM(Box::into_raw(m) as isize)).ok(); }
            });
        });
    }
}

// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    let hlb = GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32);
    let ws  = hstring(&format!("{}  {}", ts(), msg));
    SendMessageW(hlb, LB_INSERTSTRING, WPARAM(0), LPARAM(ws.as_ptr() as isize));
    if SendMessageW(hlb, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0 > 200 {
        SendMessageW(hlb, LB_DELETESTRING, WPARAM(200), LPARAM(0));
    }
    LRESULT(0)
}

unsafe fn on_app_connected(hwnd: HWND, wp: WPARAM) -> LRESULT {
    if wp.0 == 1 { set_status(hwnd, "\u{25cf}  CONNECTED"); }
    else          { set_status(hwnd, "\u{25cf}  NOT CONNECTED"); }
    EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), TRUE);
    LRESULT(0)
}

unsafe fn on_app_update(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 == 1 {
        // up to date — do nothing, button stays hidden
        return LRESULT(0);
    }
    let url = Box::from_raw(lp.0 as *mut String);
    stmut(hwnd).update_url = Some(*url);
    // Show the UPDATE button now that we know there's a newer version
    ShowWindow(GetDlgItem(hwnd, IDC_UPDATE as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_UPDATE as i32), None, TRUE);
        let ver_update = concat!("BACKUP SYNC TOOL V", env!("CARGO_PKG_VERSION"), "  \u{2191} update");
        let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_VERSION as i32),
            &hstring(ver_update));
    LRESULT(0)
}

unsafe fn on_tray(hwnd: HWND, lp: LPARAM) -> LRESULT {
    match (lp.0 & 0xFFFF) as u32 {
        WM_LBUTTONDBLCLK => { ShowWindow(hwnd, SW_SHOW); let _ = SetForegroundWindow(hwnd); }
        WM_RBUTTONUP     => tray::show_tray_menu(hwnd),
        _ => {}
    }
    LRESULT(0)
}

// ── Utilities ─────────────────────────────────────────────────────────────────
unsafe fn set_status(hwnd: HWND, t: &str) {
    let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), &hstring(t));
}

unsafe fn read_ctrls(hwnd: HWND, st: &mut WndState) {
    st.config.watch_folder        = gettext(hwnd, IDC_WATCH_FOLDER);
    st.config.webdav_url          = gettext(hwnd, IDC_URL);
    st.config.username            = gettext(hwnd, IDC_USERNAME);
    st.password_plain             = gettext(hwnd, IDC_PASSWORD);
    st.config.remote_folder       = gettext(hwnd, IDC_REMOTE_FOLDER);
    st.config.start_with_windows  = checked(hwnd, IDC_START_WINDOWS);
    st.config.sync_remote_changes = checked(hwnd, IDC_SYNC_REMOTE);
}

unsafe fn gettext(hwnd: HWND, id: u16) -> String {
    let h = GetDlgItem(hwnd, id as i32);
    let n = GetWindowTextLengthW(h); if n == 0 { return String::new(); }
    let mut b = vec![0u16; (n + 1) as usize];
    GetWindowTextW(h, &mut b);
    String::from_utf16_lossy(&b[..n as usize])
}

unsafe fn checked(hwnd: HWND, id: u16) -> bool {
    SendMessageW(GetDlgItem(hwnd, id as i32), BM_GETCHECK, WPARAM(0), LPARAM(0)).0
        == BST_CHECKED.0 as isize
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
    lf.lfHeight = h; lf.lfWeight = weight;
    let n = nw.len().min(lf.lfFaceName.len());
    lf.lfFaceName[..n].copy_from_slice(&nw[..n]);
    CreateFontIndirectW(&lf)
}

fn hstring(s: &str) -> HSTRING { HSTRING::from(s) }
fn wstr(b: &[u16]) -> String {
    let e = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..e])
}

unsafe fn msgbox(hwnd: HWND, text: &str, title: &str) {
    MessageBoxW(hwnd, &hstring(text), &hstring(title), MB_OK | MB_ICONINFORMATION);
}
unsafe fn msgbox_yn(hwnd: HWND, text: &str, title: &str) -> bool {
    MessageBoxW(hwnd, &hstring(text), &hstring(title), MB_YESNO | MB_ICONQUESTION).0
        == IDYES.0 as i32
}

fn ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{:02}:{:02}:{:02}", (s/3600)%24, (s/60)%60, s%60)
}

unsafe fn apply_startup(cfg: &Config) {
    use windows::Win32::System::Registry::*;
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut hk = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, key, 0, KEY_SET_VALUE, &mut hk).is_ok() {
        if cfg.start_with_windows {
            if let Ok(exe) = std::env::current_exe() {
                let v: Vec<u16> = exe.to_string_lossy()
                    .encode_utf16().chain(std::iter::once(0)).collect();
                let _ = RegSetValueExW(hk, w!("BackupSyncTool"), 0, REG_SZ,
                    Some(bytemuck::cast_slice(&v)));
            }
        } else {
            let _ = RegDeleteValueW(hk, w!("BackupSyncTool"));
        }
        let _ = RegCloseKey(hk);
    }
}
