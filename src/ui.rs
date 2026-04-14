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
//   Status dot:      inline on the SERVER heading row
//   Bottom bar:      version + checkboxes on one row; SAVE right
//   Spacing:         PAD=8, GAP=12, SECT=20 rhythm

use crate::config::Config;
use crate::logs;
use crate::secret;
use crate::tray;
use crate::webdav;
use std::ffi::c_void;
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::{
    DefSubclassProc, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass,
    ShellExecuteW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── Colours  0x00BBGGRR ──────────────────────────────────────────────────────
const C_WIN_BG: u32 = 0x00F0F0F0;
const C_LABEL: u32 = 0x00333333;
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
const IDC_SERVER_STATUS: u16 = 123;
const IDC_SAVE: u16 = 110;
const IDC_SYNC_STATUS: u16 = 117;
const IDC_ACTIVITY_LIST: u16 = 114;
const IDC_START_WINDOWS: u16 = 115;
const IDC_SYNC_REMOTE: u16 = 116;
// IDC_SHOW_PASSWORD (117) removed — eye icon is now drawn inside the edit subclass
const IDC_SYNC_PROGRESS: u16 = 118;
const IDC_REPO: u16 = 120;
const IDC_DEST_CREATED: u16 = 121;
const IDC_UPDATE_LINK: u16 = 122;
const IDC_PICKER_PATH: u16 = 201;
const IDC_PICKER_LIST: u16 = 202;
const IDC_PICKER_UP: u16 = 203;
const IDC_PICKER_SELECT: u16 = 205;
const IDC_PICKER_CANCEL: u16 = 206;
const IDC_SERVER_HDR: u16 = 207;
const IDC_GITHUB: u16 = 211;
const IDC_AUTHOR: u16 = 212;

const WM_APP_LOG: u32 = WM_APP + 10;
const WM_APP_CONNECTED: u32 = WM_APP + 11;
const WM_APP_UPDATE: u32 = WM_APP + 12;
const WM_APP_REMOTE_FOLDER: u32 = WM_APP + 13;
const WM_APP_PICKER_LOADED: u32 = WM_APP + 14;
const WM_APP_DEST_READY: u32 = WM_APP + 15;
const WM_APP_SYNC_ACTIVITY: u32 = WM_APP + 16;
const IDT_SYNC_ANIM: usize = 1;
const SYNC_ANIM_MS: u32 = 120;

const SS_LEFT: u32 = 0x0000;
const SS_CENTER: u32 = 0x0001;
#[allow(dead_code)]
const SS_RIGHT: u32 = 0x0002;
const SS_NOTIFY: u32 = 0x0100;

pub const CLASS_NAME: PCWSTR = w!("BackupSyncToolWnd");
const REPO_URL: &str = "https://github.com/ruibeard/backup-sync-tool";
const AUTHOR_URL: &str = "https://ruialmeida.me";
const PICKER_CLASS_NAME: PCWSTR = w!("BackupSyncToolRemotePickerWnd");
const PICKER_CLIENT_W: i32 = 430;
const PICKER_CLIENT_H: i32 = 430;

// ── Layout — 8/12/20 rhythm ──────────────────────────────────────────────────
const WIN_W: i32 = 460; // client width (slightly narrower, cleaner)
const M: i32 = 16; // outer margin
const PAD: i32 = 8; // small gap (between items in same group)
const GAP: i32 = 12; // medium gap (between rows)
const SECT: i32 = 20; // section separator gap
const INP_H: i32 = 26; // input height
const BTN_H: i32 = 30; // bottom-bar primary button height
const HDR_H: i32 = 20; // section heading height
const LBL_H: i32 = 18; // label text height
const BROWSE_W: i32 = 34; // folder icon button width
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
    sync_status_text: String,
    sync_status_state: usize,
    sync_progress_done: usize,
    sync_progress_total: usize,
    sync_started_at: Option<std::time::Instant>,
    sync_anim_frame: usize,
    sync_icon: HICON,
    sync_icon_rect: RECT,
    remote_folder_from_xd: bool,
    remote_folder_created: bool,
    /// True when URL/username/password have been edited since the last save/connect
    creds_dirty: bool,
    /// Whether the SERVER credentials section is collapsed
    server_collapsed: bool,
    /// Height of the server section controls (URL + username/password rows) when expanded
    server_section_h: i32,
    #[allow(dead_code)]
    hfont: HFONT,
    #[allow(dead_code)]
    hfont_hdr: HFONT,
    #[allow(dead_code)]
    hfont_b: HFONT,
    #[allow(dead_code)]
    hfont_small: HFONT,
    #[allow(dead_code)]
    hfont_link: HFONT,
    br_win: HBRUSH,
    br_sect: HBRUSH,
    br_input: HBRUSH,
    focused_edit: u16,
    /// Password field: is it currently showing plain text?
    pw_visible: bool,
    /// Divider y-positions for WM_PAINT
    dividers: Vec<i32>,
    /// Layout: y-position where the activity listbox starts
    activity_list_top: i32,
    /// Layout: default activity listbox height
    activity_list_h: i32,
    /// Layout: gap from bottom of listbox to sync status row
    post_list_gap: i32,
    /// Layout: height of sync status row
    sync_row_h: i32,
    /// Layout: the SECT gap after sync row
    post_sync_sect: i32,
    /// Layout: height of bottom bar area (row_h + M)
    bottom_bar_h: i32,
    /// Layout: the y of the divider between activity section and bottom bar (index in dividers)
    divider_activity_idx: usize,
    /// Layout: minimum window client height
    min_client_h: i32,
}

struct PickerResult {
    folder: Option<String>,
}

struct PickerLoadResult {
    entries: Vec<String>,
    error: Option<String>,
    resolved_folder: String,
}

struct PickerState {
    cfg: Config,
    password: String,
    current_folder: String,
    selected_folder: Option<String>,
    result: *mut PickerResult,
    hfont: HFONT,
    hfont_b: HFONT,
    busy: bool,
}

// ── Entry point ───────────────────────────────────────────────────────────────
pub fn run(hinstance: HINSTANCE, start_minimized: bool) {
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

        let picker_wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(remote_picker_wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize),
            lpszClassName: PICKER_CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&picker_wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            w!("Backup Sync Tool"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_W,
            100,
            None,
            None,
            hinstance,
            None,
        );
        ShowWindow(hwnd, if start_minimized { SW_HIDE } else { SW_SHOW });
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
            if id == IDC_SERVER_STATUS {
                SetTextColor(hdc, COLORREF(C_LABEL));
                return LRESULT((*st).br_win.0 as isize);
            }
            if id == IDC_SYNC_STATUS {
                let clr = if (*st).sync_status_state == crate::sync::ActivityState::Idle as usize {
                    C_GREEN
                } else {
                    C_LABEL
                };
                SetTextColor(hdc, COLORREF(clr));
                return LRESULT((*st).br_win.0 as isize);
            }
            let text_clr = match id {
                IDC_DEST_CREATED => C_GREEN,
                IDC_REPO => C_BLUE,
                IDC_AUTHOR => 0x00888888,
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

        WM_GETMINMAXINFO => {
            let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
            let st = state_ptr(hwnd);
            if !st.is_null() && (*st).min_client_h > 0 {
                // Calculate frame sizes
                let mut wr_test = RECT {
                    left: 0,
                    top: 0,
                    right: WIN_W,
                    bottom: (*st).min_client_h,
                };
                let _ = AdjustWindowRectEx(
                    &mut wr_test,
                    WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME,
                    false,
                    WINDOW_EX_STYLE::default(),
                );
                let frame_w = wr_test.right - wr_test.left;
                let frame_h = wr_test.bottom - wr_test.top;
                // Lock width, set min height
                mmi.ptMinTrackSize = POINT {
                    x: frame_w,
                    y: frame_h,
                };
                mmi.ptMaxTrackSize.x = frame_w; // lock horizontal
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let st = state_ptr(hwnd);
            if !st.is_null() && (*st).min_client_h > 0 {
                let mut cr = RECT::default();
                GetClientRect(hwnd, &mut cr).ok();
                let client_h = cr.bottom - cr.top;
                let extra_h = client_h - (*st).min_client_h;
                let extra = if extra_h > 0 { extra_h } else { 0 };

                // Stretch the activity listbox
                let new_lb_h = (*st).activity_list_h + extra;
                let hlb = GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32);
                SetWindowPos(
                    hlb,
                    None,
                    M,
                    (*st).activity_list_top,
                    INNER_W,
                    new_lb_h,
                    SWP_NOZORDER,
                )
                .ok();

                // Reposition sync status row
                let sync_y = (*st).activity_list_top + new_lb_h + (*st).post_list_gap;
                let sync_icon_w = 16i32;
                let sync_gap = 8i32;
                let progress_h = 10i32;
                let sync_row_h = (*st).sync_row_h;

                // Update sync icon rect for WM_PAINT
                (*st).sync_icon_rect = RECT {
                    left: M,
                    top: sync_y + (sync_row_h - sync_icon_w) / 2,
                    right: M + sync_icon_w,
                    bottom: sync_y + (sync_row_h - sync_icon_w) / 2 + sync_icon_w,
                };

                let status_x = M + sync_icon_w + sync_gap;
                let status_w = 180i32;
                let h_status = GetDlgItem(hwnd, IDC_SYNC_STATUS as i32);
                SetWindowPos(
                    h_status,
                    None,
                    status_x,
                    sync_y + (sync_row_h - LBL_H) / 2,
                    status_w,
                    LBL_H,
                    SWP_NOZORDER,
                )
                .ok();

                let progress_x = status_x + status_w + sync_gap;
                let progress_w = INNER_W - (progress_x - M);
                let h_prog = GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32);
                SetWindowPos(
                    h_prog,
                    None,
                    progress_x,
                    sync_y + (sync_row_h - progress_h) / 2,
                    progress_w,
                    progress_h,
                    SWP_NOZORDER,
                )
                .ok();

                // Update divider between activity and bottom bar
                let divider_y = sync_y + sync_row_h + (*st).post_sync_sect / 2;
                let div_idx = (*st).divider_activity_idx;
                if div_idx < (&(*st).dividers).len() {
                    (&mut (*st).dividers)[div_idx] = divider_y;
                }

                // Reposition bottom bar controls (two-row layout)
                let bottom_y = sync_y + sync_row_h + (*st).post_sync_sect;

                let row_h = BTN_H;
                let button_y = bottom_y + (row_h - BTN_H) / 2;
                let check_y = bottom_y + (row_h - 18) / 2;
                let footer_h = LBL_H;
                let footer_y = bottom_y + (row_h - footer_h) / 2;
                let save_w = 64i32;
                let update_btn_w = 26i32;
                let github_btn_w = 20i32;
                let version_w = 72i32;
                let version_x = M;
                let github_btn_x = version_x + version_w + 4;
                let update_btn_x = github_btn_x + github_btn_w + 4;
                let startup_x = update_btn_x + update_btn_w + 14;
                let two_way_x = startup_x + 78;
                let save_x = M + INNER_W - save_w;

                SetWindowPos(
                    GetDlgItem(hwnd, IDC_START_WINDOWS as i32),
                    None,
                    startup_x,
                    check_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_SYNC_REMOTE as i32),
                    None,
                    two_way_x,
                    check_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_SAVE as i32),
                    None,
                    save_x,
                    button_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_REPO as i32),
                    None,
                    version_x,
                    footer_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_GITHUB as i32),
                    None,
                    github_btn_x,
                    footer_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_UPDATE_LINK as i32),
                    None,
                    update_btn_x,
                    bottom_y + (row_h - 20) / 2,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();

                // Author row
                let author_y = bottom_y + row_h + 4;
                SetWindowPos(
                    GetDlgItem(hwnd, IDC_AUTHOR as i32),
                    None,
                    version_x,
                    author_y,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE,
                )
                .ok();

                InvalidateRect(hwnd, None, TRUE);
            }
            LRESULT(0)
        }

        tray::WM_TRAY => on_tray(hwnd, lparam),
        WM_APP_LOG => on_app_log(hwnd, lparam),
        WM_APP_CONNECTED => on_app_connected(hwnd, wparam),
        WM_APP_UPDATE => on_app_update(hwnd, wparam, lparam),
        WM_APP_REMOTE_FOLDER => on_app_remote_folder(hwnd, lparam),
        WM_APP_DEST_READY => on_app_dest_ready(hwnd, wparam),
        WM_APP_SYNC_ACTIVITY => on_app_sync_activity(hwnd, wparam, lparam),
        WM_TIMER => on_timer(hwnd, wparam),

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

    if (*st).sync_icon.0 != 0 {
        let r = (*st).sync_icon_rect;
        let _ = DrawIconEx(
            hdc,
            r.left,
            r.top,
            (*st).sync_icon,
            r.right - r.left,
            r.bottom - r.top,
            0,
            HBRUSH(0),
            DI_NORMAL,
        );
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
    let hfont_link = mkfont_underline("Segoe UI", 9, FW_NORMAL.0 as i32);

    let mut cfg = crate::config::load();
    let mut remote_folder_from_xd = false;
    if cfg.watch_folder.is_empty() {
        if let Some(path) = crate::xd::default_watch_folder() {
            cfg.watch_folder = path;
        }
    }
    if cfg.remote_folder.is_empty() {
        if let Some(remote_folder) = crate::xd::detect_default_remote_folder() {
            cfg.remote_folder = remote_folder;
            remote_folder_from_xd = true;
        }
    }
    let pass = secret::decrypt(&cfg.password_enc).unwrap_or_default();

    let state = Box::new(WndState {
        config: cfg.clone(),
        password_plain: pass.clone(),
        sync_engine: None,
        update_url: None,
        connected: false,
        sync_status_text: "Checking...".to_string(),
        sync_status_state: crate::sync::ActivityState::Checking as usize,
        sync_progress_done: 0,
        sync_progress_total: 0,
        sync_started_at: None,
        sync_anim_frame: 0,
        sync_icon: HICON(0),
        sync_icon_rect: RECT::default(),
        remote_folder_from_xd,
        remote_folder_created: false,
        creds_dirty: false,
        server_collapsed: true,
        server_section_h: 0,
        hfont,
        hfont_hdr,
        hfont_b,
        hfont_small,
        hfont_link,
        br_win: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_sect: CreateSolidBrush(COLORREF(C_WIN_BG)),
        br_input: CreateSolidBrush(COLORREF(C_INPUT_BG)),
        focused_edit: 0,
        pw_visible: false,
        dividers: Vec::new(),
        activity_list_top: 0,
        activity_list_h: 0,
        post_list_gap: 0,
        sync_row_h: 0,
        post_sync_sect: 0,
        bottom_bar_h: 0,
        divider_activity_idx: 0,
        min_client_h: 0,
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
        hfont_link,
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
    let log: crate::sync::LogFn = Arc::new(move |m: String| {
        logs::append(&m);
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
    let activity: crate::sync::ActivityFn = Arc::new(move |info| unsafe {
        PostMessageW(
            HWND(raw),
            WM_APP_SYNC_ACTIVITY,
            WPARAM(info.state as usize),
            LPARAM(Box::into_raw(Box::new((info.completed, info.total))) as isize),
        )
        .ok();
    });

    if !cfg.watch_folder.is_empty()
        && !cfg.webdav_url.is_empty()
        && !cfg.username.is_empty()
        && !pass.is_empty()
        && !cfg.remote_folder.is_empty()
    {
        match crate::sync::SyncEngine::start(
            cfg.clone(),
            pass.clone(),
            log.clone(),
            activity.clone(),
        ) {
            Ok(engine) => stmut(hwnd).sync_engine = Some(engine),
            Err(err) => {
                let msg = Box::new(format!("Sync start failed: {err}"));
                PostMessageW(
                    HWND(raw),
                    WM_APP_LOG,
                    WPARAM(0),
                    LPARAM(Box::into_raw(msg) as isize),
                )
                .ok();
            }
        }
    }

    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_HIDE);
        let cfg2 = cfg.clone();
        let pass2 = pass.clone();
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
            &hstring("Connecting"),
        );
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

    std::thread::spawn(
        move || match crate::updater::check(env!("CARGO_PKG_VERSION")) {
            crate::updater::CheckResult::UpdateAvailable(info) => {
                let url = Box::new(info.url);
                PostMessageW(
                    HWND(raw),
                    WM_APP_UPDATE,
                    WPARAM(0),
                    LPARAM(Box::into_raw(url) as isize),
                )
                .ok();
            }
            crate::updater::CheckResult::UpToDate => {}
            crate::updater::CheckResult::Error(e) => {
                crate::logs::append(&format!("Update check error: {e}"));
            }
        },
    );
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
    hf_link: HFONT,
) {
    let st = &mut *state_ptr(hwnd);
    let mut y = M + 4;

    // ── SERVER ────────────────────────────────────────────────────────────────
    {
        let status_w = 16i32;
        let server_status_w = 84i32;
        let server_status_x = M + INNER_W - server_status_w;
        let status_x = server_status_x - status_w - 4;

        // Clickable header row: "▶ SERVER" toggle + status dot + status text
        let hdr_toggle_w = 90i32;
        mklink(
            hwnd,
            hi,
            IDC_SERVER_HDR,
            "\u{25B6}  SERVER",
            M,
            y,
            hdr_toggle_w,
            HDR_H,
            hf_hdr,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_SERVER_STATUS,
            "Not connected",
            server_status_x,
            y,
            server_status_w,
            LBL_H,
            hf_small,
            SS_RIGHT,
        );
        mkstatic_align(
            hwnd,
            hi,
            IDC_STATUS_TEXT,
            "\u{25cf}",
            status_x,
            y,
            status_w,
            LBL_H,
            hf_small,
            SS_CENTER,
        );
        y += HDR_H + PAD;

        // --- Collapsible controls (hidden by default) ---
        let section_top = y;
        let url_w = INNER_W;

        mkedit_cue(
            hwnd,
            hi,
            IDC_URL,
            &cfg.webdav_url,
            "https://example.com/webdav",
            M,
            y,
            url_w,
            hf,
        );

        // Connect button overlays URL row right edge
        mkbtn_blue(
            hwnd,
            hi,
            IDC_CONNECT,
            "Connect",
            M + INNER_W - 54,
            y,
            54,
            INP_H,
            hf_b,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
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

        // Record how tall this section is so toggle can shift everything below
        st.server_section_h = y - section_top;

        // Hide the controls if starting collapsed
        let server_ctrl_ids = [IDC_URL, IDC_USERNAME, IDC_PASSWORD];
        for &ctrl_id in &server_ctrl_ids {
            ShowWindow(GetDlgItem(hwnd, ctrl_id as i32), SW_HIDE);
        }
        // Also hide the two field labels — they have no ID so we hide by position via EnumChildWindows.
        // Simpler: we give them IDs. Use IDC_LBL_URL=208, IDC_LBL_USER=209, IDC_LBL_PASS=210.
        // But those were created with mkfield_label which doesn't take an id.
        // Instead, collapse by adjusting y back and not creating labels when collapsed.
        // Re-do: create labels with IDs so we can hide them.
        // We already created them above without IDs — destroy & recreate with IDs below.
        // Actually the simplest approach: just reduce y back to section_top, we'll
        // handle the show/hide of all server controls in toggle_server_section.
        if st.server_collapsed {
            y = section_top;
        }

        st.dividers
            .push(y - SECT / 2 + if st.server_collapsed { SECT / 2 } else { 0 });
    }

    // ── FOLDERS ───────────────────────────────────────────────────────────────
    {
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
            "...",
            browse_x,
            y,
            34,
            INP_H,
            hf,
        );
        y += INP_H + GAP;

        mkfield_label(hwnd, hi, "Destination folder", M, y, 112, hf_small);
        mkstatic(
            hwnd,
            hi,
            IDC_DEST_CREATED,
            "Created on server",
            M + 118,
            y,
            120,
            LBL_H,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
        y += LBL_H + 4;
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
            "...",
            browse_x,
            y,
            34,
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

        let lb_h = 140i32;
        st.activity_list_top = y;
        st.activity_list_h = lb_h;
        mklb(hwnd, hi, IDC_ACTIVITY_LIST, M, y, INNER_W, lb_h, hf_small);
        y += lb_h;
        st.post_list_gap = PAD;
        y += PAD;

        // Sync status row (icon + text + progress) below activity list
        let sync_icon_w = 16i32;
        let sync_gap = 8i32;
        let progress_h = 10;
        let sync_row_h = progress_h + 4;
        st.sync_row_h = sync_row_h;

        // Store icon rect for WM_PAINT
        st.sync_icon_rect = RECT {
            left: M,
            top: y + (sync_row_h - sync_icon_w) / 2,
            right: M + sync_icon_w,
            bottom: y + (sync_row_h - sync_icon_w) / 2 + sync_icon_w,
        };

        let idle_icon = LoadIconW(hi, w!("APP_ICON_IDLE")).unwrap_or_default();
        st.sync_icon = idle_icon;

        let status_x = M + sync_icon_w + sync_gap;
        let status_w = 180i32;
        mkstatic_align(
            hwnd,
            hi,
            IDC_SYNC_STATUS,
            &st.sync_status_text,
            status_x,
            y + (sync_row_h - LBL_H) / 2,
            status_w,
            LBL_H,
            hf_small,
            SS_LEFT,
        );

        // Progress bar to the right of status
        let progress_x = status_x + status_w + sync_gap;
        let progress_w = INNER_W - (progress_x - M);
        mkprogress(
            hwnd,
            hi,
            IDC_SYNC_PROGRESS,
            progress_x,
            y + (sync_row_h - progress_h) / 2,
            progress_w,
            progress_h,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32), SW_HIDE);

        y += sync_row_h;
        st.post_sync_sect = SECT;
        y += SECT;

        st.divider_activity_idx = st.dividers.len();
        st.dividers.push(y - SECT / 2);
    }

    // ── BOTTOM BAR ────────────────────────────────────────────────────────────
    // Row 1: version + github icon + update + checkboxes + save
    // Row 2: author credit
    {
        let row_h = BTN_H;
        let button_y = y + (row_h - BTN_H) / 2;
        let check_y = y + (row_h - 18) / 2;
        let footer_h = LBL_H;
        let footer_y = y + (row_h - footer_h) / 2;
        let save_w = 64i32;
        let update_btn_w = 26i32;
        let update_btn_h = 20i32;
        let github_btn_w = 20i32;
        let version_w = 72i32;
        let version_x = M;
        let github_btn_x = version_x + version_w + 4;
        let update_btn_x = github_btn_x + github_btn_w + 4;
        let startup_x = update_btn_x + update_btn_w + 14;
        let two_way_x = startup_x + 78;
        let save_x = M + INNER_W - save_w;
        let update_btn_y = y + (row_h - update_btn_h) / 2;

        mkcheck(
            hwnd,
            hi,
            IDC_START_WINDOWS,
            "Startup",
            startup_x,
            check_y,
            70,
            18,
            hf_small,
            cfg.start_with_windows,
        );
        mkcheck(
            hwnd,
            hi,
            IDC_SYNC_REMOTE,
            "Two-way sync",
            two_way_x,
            check_y,
            100,
            18,
            hf_small,
            cfg.sync_remote_changes,
        );

        mkbtn_blue(
            hwnd, hi, IDC_SAVE, "Save", save_x, button_y, save_w, BTN_H, hf_b,
        );
        let ver_label = concat!("v", env!("CARGO_PKG_VERSION"));

        mklink(
            hwnd, hi, IDC_REPO, ver_label, version_x, footer_y, version_w, footer_h, hf_link,
        );

        // GitHub icon button (owner-drawn, draws GitHub octocat-like icon)
        mkbtn(
            hwnd,
            hi,
            IDC_GITHUB,
            "",
            github_btn_x,
            footer_y,
            github_btn_w,
            footer_h,
            hf_small,
        );

        mkbtn(
            hwnd,
            hi,
            IDC_UPDATE_LINK,
            "",
            update_btn_x,
            update_btn_y,
            update_btn_w,
            update_btn_h,
            hf_small,
        );
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);

        y += row_h;

        // Author credit row
        let author_h = LBL_H - 2;
        let author_y = y + 4;
        mklink(
            hwnd,
            hi,
            IDC_AUTHOR,
            "Rui Almeida \u{00B7} ruialmeida.me",
            version_x,
            author_y,
            200,
            author_h,
            hf_link,
        );
        y += author_h + 4 + M;
        st.bottom_bar_h = y - (y - row_h - author_h - 4 - M);
    }

    // Size window to fit content
    st.min_client_h = y;
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

unsafe fn mklink(
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
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_NOTIFY | SS_LEFT),
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

unsafe fn mkstatic_align(
    hwnd: HWND,
    hi: HINSTANCE,
    id: u16,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hf: HFONT,
    align: u32,
) -> HWND {
    let hs = hstring(text);
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        &hs,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(align),
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

unsafe fn mkbtn_std(
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
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
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

unsafe fn mkprogress(hwnd: HWND, hi: HINSTANCE, id: u16, x: i32, y: i32, w: i32, h: i32) -> HWND {
    let c = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("msctls_progress32"),
        w!(""),
        WS_CHILD | WS_VISIBLE,
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as isize),
        hi,
        None,
    );
    SendMessageW(c, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
    c
}

const C_FOLDER_FILL: u32 = 0x00A5C8ED; // light tan/beige folder fill (BGR for #EDC8A5)
const C_FOLDER_LINE: u32 = 0x00607890; // darker outline for folder (BGR for #907860)

// ── WM_DRAWITEM ───────────────────────────────────────────────────────────────
const BLUE_IDS: &[u16] = &[IDC_CONNECT, IDC_SAVE, IDC_UPDATE_LINK];
const BORDERLESS_IDS: &[u16] = &[IDC_BROWSE_LOCAL, IDC_BROWSE_REMOTE, IDC_GITHUB];
const FOLDER_IDS: &[u16] = &[IDC_BROWSE_LOCAL, IDC_BROWSE_REMOTE];
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
        // Draw a GitHub icon via GDI
        draw_github_icon(hdc, &rc, C_GREY_TXT);
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

/// Draw a simplified GitHub-style icon (circle with a small fork/branch symbol).
/// Renders clearly at small sizes like 14×14.
unsafe fn draw_github_icon(hdc: HDC, rc: &RECT, clr: u32) {
    let cx = (rc.left + rc.right) / 2;
    let cy = (rc.top + rc.bottom) / 2;
    let r = 6i32; // outer circle radius

    let hp = CreatePen(PS_SOLID, 1, COLORREF(clr));
    let op = SelectObject(hdc, hp);
    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));

    // Outer circle
    Ellipse(hdc, cx - r, cy - r, cx + r + 1, cy + r + 1);

    // Branch/fork symbol inside: vertical line + two arms up
    SelectObject(hdc, GetStockObject(NULL_BRUSH));
    let hp2 = CreatePen(PS_SOLID, 1, COLORREF(clr));
    let op2 = SelectObject(hdc, hp2);

    // Vertical trunk
    MoveToEx(hdc, cx, cy - 3, None);
    LineTo(hdc, cx, cy + 3);

    // Left arm (diagonal up-left)
    MoveToEx(hdc, cx, cy - 1, None);
    LineTo(hdc, cx - 2, cy - 3);

    // Right arm (diagonal up-right)
    MoveToEx(hdc, cx, cy - 1, None);
    LineTo(hdc, cx + 2, cy - 3);

    // Small dot at top-left tip
    let pb = CreateSolidBrush(COLORREF(clr));
    let opb = SelectObject(hdc, pb);
    Ellipse(hdc, cx - 3, cy - 5, cx - 1, cy - 3);
    // Small dot at top-right tip
    Ellipse(hdc, cx + 1, cy - 5, cx + 3, cy - 3);
    // Small dot at bottom
    Ellipse(hdc, cx - 1, cy + 2, cx + 1, cy + 4);
    SelectObject(hdc, opb);
    DeleteObject(pb);

    SelectObject(hdc, op2);
    DeleteObject(hp2);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    DeleteObject(hp);
}

unsafe fn on_command(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let id = (wp.0 & 0xFFFF) as u16;
    let notif = (wp.0 >> 16) as u16;

    // EN_CHANGE on credential fields → mark dirty, show Connect button, hide status
    if notif == 0x0300u16 && (id == IDC_URL || id == IDC_USERNAME || id == IDC_PASSWORD) {
        let st = stmut(hwnd);
        if !st.creds_dirty {
            st.creds_dirty = true;
            st.connected = false;
            ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_HIDE);
            let _ = SetWindowTextW(
                GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
                &hstring("Needs connect"),
            );
            ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_SHOW);
            EnableWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), TRUE);
        }
        return LRESULT(0);
    }

    if notif == 0x0300u16 && id == IDC_REMOTE_FOLDER {
        let st = stmut(hwnd);
        st.remote_folder_from_xd = false;
        st.remote_folder_created = false;
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
        return LRESULT(0);
    }

    if notif == STN_CLICKED as u16 {
        match id {
            IDC_REPO => {
                do_open_repo(hwnd);
                return LRESULT(0);
            }
            IDC_AUTHOR => {
                do_open_author(hwnd);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    match id {
        x if x == tray::ID_TRAY_OPEN as u16 => {
            ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        x if x == tray::ID_TRAY_LOGS as u16 => {
            do_open_logs(hwnd);
        }
        x if x == tray::ID_TRAY_EXIT as u16 => {
            DestroyWindow(hwnd).ok();
        }
        IDC_BROWSE_LOCAL => browse_local(hwnd),
        IDC_BROWSE_REMOTE => browse_remote(hwnd),
        IDC_CONNECT => do_connect(hwnd),
        IDC_SAVE => do_save(hwnd),
        IDC_UPDATE_LINK => do_update(hwnd),
        IDC_GITHUB => do_open_repo(hwnd),
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
    if let Err(err) = validate_webdav_url(&cfg.webdav_url) {
        msgbox(hwnd, &err, "Connect");
        return;
    }
    let pass = st.password_plain.clone();
    ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
    // Show amber/yellow dot while connecting (color set via WM_CTLCOLORSTATIC)
    set_status(hwnd, "\u{25cf}");
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SERVER_STATUS as i32),
        &hstring("Connecting"),
    );
    ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
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
    if st.config.watch_folder.trim().is_empty() {
        msgbox(hwnd, "Origin folder is required.", "Save");
        return;
    }
    if st.config.webdav_url.trim().is_empty() {
        msgbox(hwnd, "Server URL is required.", "Save");
        return;
    }
    if let Err(err) = validate_webdav_url(&st.config.webdav_url) {
        msgbox(hwnd, &err, "Save");
        return;
    }
    if st.config.remote_folder.trim().is_empty() {
        msgbox(hwnd, "Destination folder is required.", "Save");
        return;
    }
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
        logs::append(&m);
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
    let activity: crate::sync::ActivityFn = Arc::new(move |info| unsafe {
        PostMessageW(
            HWND(raw),
            WM_APP_SYNC_ACTIVITY,
            WPARAM(info.state as usize),
            LPARAM(Box::into_raw(Box::new((info.completed, info.total))) as isize),
        )
        .ok();
    });
    if st.sync_engine.is_some() {
        st.sync_engine = None;
    }
    match crate::sync::SyncEngine::start(cfg.clone(), pass.clone(), log, activity) {
        Ok(e) => {
            let st = stmut(hwnd);
            st.sync_engine = Some(e);
            let msg = Box::new("Settings saved. File watching is active.".to_string());
            PostMessageW(
                HWND(raw),
                WM_APP_LOG,
                WPARAM(0),
                LPARAM(Box::into_raw(msg) as isize),
            )
            .ok();
            msgbox(hwnd, "Settings saved. Sync is now active.", "Save");
        }
        Err(e) => {
            msgbox(hwnd, &format!("Sync error: {e}"), "Error");
        }
    }
    if !cfg.webdav_url.is_empty() && !cfg.username.is_empty() && !pass.is_empty() {
        ShowWindow(GetDlgItem(hwnd, IDC_CONNECT as i32), SW_HIDE);
        set_status(hwnd, "\u{25cf}"); // connecting dot
        ShowWindow(GetDlgItem(hwnd, IDC_STATUS_TEXT as i32), SW_SHOW);
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
        let msg = Box::new("Update started.".to_string());
        PostMessageW(
            hwnd,
            WM_APP_LOG,
            WPARAM(0),
            LPARAM(Box::into_raw(msg) as isize),
        )
        .ok();
        ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_HIDE);
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

unsafe fn do_open_author(hwnd: HWND) {
    let _ = ShellExecuteW(
        hwnd,
        w!("open"),
        &hstring(AUTHOR_URL),
        None,
        None,
        SW_SHOWNORMAL,
    );
}

unsafe fn do_open_logs(hwnd: HWND) {
    let dir = logs::ensure_logs_dir();
    let dir_w = hstring(&dir.to_string_lossy());
    let _ = ShellExecuteW(hwnd, w!("open"), &dir_w, None, None, SW_SHOWNORMAL);
}

// ── App messages ──────────────────────────────────────────────────────────────
unsafe fn on_app_log(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let msg = Box::from_raw(lp.0 as *mut String);
    let Some(entry) = activity_entry(&msg) else {
        return LRESULT(0);
    };
    let hlb = GetDlgItem(hwnd, IDC_ACTIVITY_LIST as i32);
    let ws = hstring(&entry);
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

unsafe fn on_app_sync_activity(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let progress = if lp.0 != 0 {
        *Box::from_raw(lp.0 as *mut (usize, usize))
    } else {
        (0, 0)
    };
    let (icon_name, mut status_text) = match wp.0 {
        x if x == crate::sync::ActivityState::Checking as usize => {
            (w!("APP_ICON_IDLE"), "Checking...")
        }
        x if x == crate::sync::ActivityState::Syncing as usize => {
            (w!("APP_ICON_SYNCING"), "Syncing...")
        }
        _ => (w!("APP_ICON_COMPLETE"), "All synced"),
    };

    let st = stmut(hwnd);
    let was_syncing = st.sync_status_state == crate::sync::ActivityState::Syncing as usize;
    st.sync_status_state = wp.0;
    st.sync_progress_done = progress.0;
    st.sync_progress_total = progress.1;
    if wp.0 == crate::sync::ActivityState::Syncing as usize {
        if !was_syncing {
            st.sync_started_at = Some(std::time::Instant::now());
        }
        if progress.1 > 0 {
            let done = progress.0.min(progress.1);
            let pct = (done * 100) / progress.1;
            let eta = if done > 0 {
                st.sync_started_at.and_then(|started| {
                    let elapsed = started.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        let per_item = elapsed / done as f64;
                        let remaining = ((progress.1 - done) as f64 * per_item).ceil() as u64;
                        Some(format_eta(remaining))
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            st.sync_status_text = if let Some(eta) = eta {
                format!("{done}/{} \u{00B7} ETA {} \u{00B7} {pct}%", progress.1, eta)
            } else {
                format!("{done}/{} \u{00B7} {pct}%", progress.1)
            };
            status_text = &st.sync_status_text;
        }
        if !was_syncing {
            st.sync_anim_frame = 0;
            let _ = SetTimer(hwnd, IDT_SYNC_ANIM, SYNC_ANIM_MS, None);
        }
    } else {
        st.sync_started_at = None;
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as isize);
        let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
        if hicon.0 != 0 {
            tray::set_tray_icon_and_tip(hwnd, hicon, "Backup Sync Tool");
            st.sync_icon = hicon;
            InvalidateRect(hwnd, Some(&st.sync_icon_rect), TRUE);
        }
    }
    if wp.0 != crate::sync::ActivityState::Syncing as usize {
        st.sync_status_text = status_text.to_string();
    }
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_SYNC_STATUS as i32),
        &hstring(&st.sync_status_text),
    );
    let progress_hwnd = GetDlgItem(hwnd, IDC_SYNC_PROGRESS as i32);
    if wp.0 == crate::sync::ActivityState::Syncing as usize && progress.1 > 0 {
        let pct = ((progress.0.min(progress.1) * 100) / progress.1) as isize;
        SendMessageW(progress_hwnd, PBM_SETPOS, WPARAM(pct as usize), LPARAM(0));
        ShowWindow(progress_hwnd, SW_SHOW);
        let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as isize);
        let tip_icon = LoadIconW(hi, w!("APP_ICON_SYNCING")).unwrap_or_default();
        if tip_icon.0 != 0 {
            tray::set_tray_icon_and_tip(
                hwnd,
                tip_icon,
                &format!("Backup Sync Tool - {}", st.sync_status_text),
            );
        }
    } else {
        SendMessageW(progress_hwnd, PBM_SETPOS, WPARAM(0), LPARAM(0));
        ShowWindow(progress_hwnd, SW_HIDE);
    }
    InvalidateRect(GetDlgItem(hwnd, IDC_SYNC_STATUS as i32), None, TRUE);
    LRESULT(0)
}

unsafe fn on_timer(hwnd: HWND, wp: WPARAM) -> LRESULT {
    if wp.0 != IDT_SYNC_ANIM {
        return DefWindowProcW(hwnd, WM_TIMER, wp, LPARAM(0));
    }

    let st = stmut(hwnd);
    if st.sync_status_state != crate::sync::ActivityState::Syncing as usize {
        let _ = KillTimer(hwnd, IDT_SYNC_ANIM);
        return LRESULT(0);
    }

    let names = [
        w!("APP_ICON_SYNC_1"),
        w!("APP_ICON_SYNC_2"),
        w!("APP_ICON_SYNC_3"),
        w!("APP_ICON_SYNC_4"),
        w!("APP_ICON_SYNC_5"),
        w!("APP_ICON_SYNC_6"),
    ];
    let hi = HINSTANCE(GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as isize);
    let icon_name = names[st.sync_anim_frame % names.len()];
    st.sync_anim_frame = (st.sync_anim_frame + 1) % names.len();
    let hicon = LoadIconW(hi, icon_name).unwrap_or_default();
    if hicon.0 != 0 {
        let tip = if !st.sync_status_text.is_empty() {
            format!("Backup Sync Tool - {}", st.sync_status_text)
        } else {
            "Backup Sync Tool - Syncing".to_string()
        };
        tray::set_tray_icon_and_tip(hwnd, hicon, &tip);
        st.sync_icon = hicon;
        InvalidateRect(hwnd, Some(&st.sync_icon_rect), TRUE);
    }
    LRESULT(0)
}

unsafe fn on_app_connected(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let connected = wp.0 == 1;
    let st = stmut(hwnd);
    st.connected = connected;
    let status_hwnd = GetDlgItem(hwnd, IDC_STATUS_TEXT as i32);
    let status_label_hwnd = GetDlgItem(hwnd, IDC_SERVER_STATUS as i32);
    let conn_hwnd = GetDlgItem(hwnd, IDC_CONNECT as i32);
    if connected {
        set_status(hwnd, "\u{25cf}"); // Just the dot - green = connected
        let _ = SetWindowTextW(status_label_hwnd, &hstring("Connected"));
        st.creds_dirty = false;
        ShowWindow(conn_hwnd, SW_HIDE);
        ShowWindow(status_hwnd, SW_SHOW);
        maybe_create_xd_remote_folder(hwnd);
    } else {
        set_status(hwnd, "\u{25cf}"); // Just the dot - red = not connected
        let _ = SetWindowTextW(status_label_hwnd, &hstring("Offline"));
        EnableWindow(conn_hwnd, TRUE);
        ShowWindow(conn_hwnd, SW_SHOW);
        ShowWindow(status_hwnd, SW_SHOW);
    }
    InvalidateRect(status_hwnd, None, TRUE);
    LRESULT(0)
}

unsafe fn on_app_update(hwnd: HWND, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if wp.0 == 1 {
        return LRESULT(0);
    }
    let url = Box::from_raw(lp.0 as *mut String);
    stmut(hwnd).update_url = Some(*url);
    ShowWindow(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), SW_SHOW);
    InvalidateRect(GetDlgItem(hwnd, IDC_UPDATE_LINK as i32), None, TRUE);
    LRESULT(0)
}

unsafe fn on_app_remote_folder(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let remote_folder = Box::from_raw(lp.0 as *mut String);
    if gettext(hwnd, IDC_REMOTE_FOLDER).is_empty() {
        let st = stmut(hwnd);
        st.config.remote_folder = (*remote_folder).clone();
        st.remote_folder_from_xd = true;
        st.remote_folder_created = false;
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&remote_folder),
        );
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
    }
    LRESULT(0)
}

unsafe fn on_app_dest_ready(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let created = wp.0 == 1;
    let st = stmut(hwnd);
    st.remote_folder_created = created;
    if created {
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_SHOW);
        InvalidateRect(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), None, TRUE);
    } else {
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
    }
    LRESULT(0)
}

unsafe fn browse_remote(hwnd: HWND) {
    let st = stmut(hwnd);
    read_ctrls(hwnd, st);

    if st.config.webdav_url.trim().is_empty()
        || st.config.username.trim().is_empty()
        || st.password_plain.trim().is_empty()
    {
        msgbox(
            hwnd,
            "Fill Server URL, Username, and Password first.",
            "Remote Folder",
        );
        return;
    }

    if let Some(folder) = remote_folder_picker(hwnd, st.config.clone(), st.password_plain.clone()) {
        st.config.remote_folder = folder.clone();
        st.remote_folder_from_xd = false;
        st.remote_folder_created = false;
        ShowWindow(GetDlgItem(hwnd, IDC_DEST_CREATED as i32), SW_HIDE);
        let _ = SetWindowTextW(
            GetDlgItem(hwnd, IDC_REMOTE_FOLDER as i32),
            &hstring(&folder),
        );
    }
}

unsafe fn maybe_create_xd_remote_folder(hwnd: HWND) {
    let st = stmut(hwnd);
    if !st.remote_folder_from_xd || st.remote_folder_created {
        return;
    }
    if st.config.remote_folder.trim().is_empty()
        || st.config.webdav_url.trim().is_empty()
        || st.config.username.trim().is_empty()
        || st.password_plain.trim().is_empty()
    {
        return;
    }

    let cfg = st.config.clone();
    let pass = st.password_plain.clone();
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let created = ensure_remote_folder_exists(&cfg, &pass, &cfg.remote_folder).is_ok();
        unsafe {
            PostMessageW(
                HWND(raw),
                WM_APP_DEST_READY,
                WPARAM(if created { 1 } else { 0 }),
                LPARAM(0),
            )
            .ok();
        }
    });
}

fn ensure_remote_folder_exists(
    cfg: &Config,
    password: &str,
    folder: &str,
) -> std::result::Result<(), String> {
    let folder = normalize_remote_folder(folder);
    if folder.is_empty() {
        return Ok(());
    }

    let mut current = String::new();
    for part in folder.split('/') {
        if current.is_empty() {
            current.push_str(part);
        } else {
            current.push('/');
            current.push_str(part);
        }
        let url = join_remote_url(&cfg.webdav_url, &current);
        webdav::mkcol(cfg, password, &url)?;
    }
    Ok(())
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

unsafe fn mkfont_underline(name: &str, pt: i32, weight: i32) -> HFONT {
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
    ReleaseDC(None, hdc);
    let h = -(pt * dpi / 72);
    let nw: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW::default();
    lf.lfHeight = h;
    lf.lfWeight = weight;
    lf.lfUnderline = 1; // underline for clickable links
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

fn activity_entry(message: &str) -> Option<String> {
    if message.starts_with("Checking remote files") {
        return Some(message.to_string());
    }
    if message.starts_with("Counting local files") {
        return Some(message.to_string());
    }
    if message.starts_with("Comparing local to remote") {
        return Some(message.to_string());
    }
    if message.starts_with("Checking remote changes") {
        return Some(message.to_string());
    }
    if let Some(name) = message.strip_prefix("Uploaded: ") {
        return Some(format!("↑ {}", display_activity_name(name)));
    }
    if let Some(name) = message.strip_prefix("Downloaded: ") {
        return Some(format!("↓ {}", display_activity_name(name)));
    }
    None
}

fn display_activity_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn format_eta(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

fn validate_webdav_url(url: &str) -> std::result::Result<(), String> {
    if url.trim().to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err("Server URL must start with https://".to_string())
    }
}

unsafe fn remote_folder_picker(hwnd: HWND, cfg: Config, password: String) -> Option<String> {
    let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
    let hfont = mkfont("Segoe UI", 11, FW_NORMAL.0 as i32);
    let hfont_b = mkfont("Segoe UI", 11, FW_SEMIBOLD.0 as i32);
    let current = normalize_remote_folder(&cfg.remote_folder);

    let result = Box::new(PickerResult { folder: None });
    let result_ptr = Box::into_raw(result);
    let state = Box::new(PickerState {
        cfg,
        password,
        current_folder: current.clone(),
        selected_folder: Some(current),
        result: result_ptr,
        hfont,
        hfont_b,
        busy: false,
    });

    let picker = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        PICKER_CLASS_NAME,
        w!("Select Destination Folder"),
        WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        100,
        100,
        hwnd,
        None,
        hinstance,
        Some(Box::into_raw(state) as *const c_void),
    );

    if picker.0 == 0 {
        let _ = Box::from_raw(result_ptr);
        return None;
    }

    let mut rc = RECT {
        left: 0,
        top: 0,
        right: PICKER_CLIENT_W,
        bottom: PICKER_CLIENT_H,
    };
    AdjustWindowRectEx(
        &mut rc,
        WS_CAPTION | WS_SYSMENU | WS_POPUP,
        false,
        WS_EX_DLGMODALFRAME,
    )
    .ok();
    SetWindowPos(
        picker,
        None,
        0,
        0,
        rc.right - rc.left,
        rc.bottom - rc.top,
        SWP_NOMOVE | SWP_NOZORDER,
    )
    .ok();

    EnableWindow(hwnd, FALSE);
    ShowWindow(picker, SW_SHOW);
    UpdateWindow(picker);

    let mut msg = MSG::default();
    while IsWindow(picker).as_bool() && GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        if !IsDialogMessageW(picker, &msg).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    EnableWindow(hwnd, TRUE);
    let _ = SetForegroundWindow(hwnd);

    let result = Box::from_raw(result_ptr);
    result.folder.clone()
}

unsafe extern "system" fn remote_picker_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(1)
        }
        WM_CREATE => {
            picker_on_create(hwnd);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wp.0 as isize);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(C_LABEL));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_COMMAND => picker_on_command(hwnd, wp),
        WM_APP_PICKER_LOADED => picker_on_loaded(hwnd, lp),
        WM_CLOSE => {
            DestroyWindow(hwnd).ok();
            LRESULT(0)
        }
        WM_DESTROY => {
            picker_on_destroy(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn picker_on_create(hwnd: HWND) {
    let st = picker_state(hwnd);
    let hi: HINSTANCE = GetModuleHandleW(None).unwrap().into();
    let margin = 12;
    let width = 430 - margin * 2;
    let path_label_y = 12;
    let path_y = 32;
    let list_label_y = 68;
    let list_y = 88;
    let list_h = 228;
    let button_y = 332;

    mkfield_label(
        hwnd,
        hi,
        "Current folder",
        margin,
        path_label_y,
        width,
        st.hfont,
    );

    let path = mkedit_raw(
        hwnd,
        hi,
        IDC_PICKER_PATH,
        &display_picker_folder(&st.current_folder),
        margin,
        path_y,
        width,
        st.hfont,
    );
    SendMessageW(path, EM_SETREADONLY, WPARAM(1), LPARAM(0));

    mkfield_label(hwnd, hi, "Folders", margin, list_label_y, width, st.hfont);

    mklb(
        hwnd,
        hi,
        IDC_PICKER_LIST,
        margin,
        list_y,
        width,
        list_h,
        st.hfont,
    );

    mkbtn_std(
        hwnd,
        hi,
        IDC_PICKER_UP,
        "Up",
        margin,
        button_y,
        70,
        BTN_H,
        st.hfont,
    );
    mkbtn_std(
        hwnd,
        hi,
        IDC_PICKER_CANCEL,
        "Cancel",
        margin + width - 170,
        button_y,
        80,
        BTN_H,
        st.hfont,
    );
    mkbtn_std(
        hwnd,
        hi,
        IDC_PICKER_SELECT,
        "Select",
        margin + width - 82,
        button_y,
        80,
        BTN_H,
        st.hfont_b,
    );

    picker_load_current(hwnd);
}

unsafe fn picker_on_command(hwnd: HWND, wp: WPARAM) -> LRESULT {
    let id = (wp.0 & 0xFFFF) as u16;
    let notif = (wp.0 >> 16) as u16;

    match id {
        IDC_PICKER_UP => {
            picker_go_up(hwnd);
        }
        IDC_PICKER_SELECT => {
            picker_commit(hwnd);
        }
        IDC_PICKER_CANCEL => {
            DestroyWindow(hwnd).ok();
        }
        IDC_PICKER_LIST => {
            if notif == LBN_SELCHANGE as u16 {
                picker_select_current_list_item(hwnd);
            } else if notif == LBN_DBLCLK as u16 {
                picker_enter_current_list_item(hwnd);
            }
        }
        _ => {}
    }
    LRESULT(0)
}

unsafe fn picker_on_loaded(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let result = Box::from_raw(lp.0 as *mut PickerLoadResult);
    let st = picker_state(hwnd);
    st.busy = false;
    st.current_folder = result.resolved_folder.clone();
    if st.selected_folder.is_none() {
        st.selected_folder = Some(st.current_folder.clone());
    }
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PICKER_PATH as i32),
        &hstring(&display_picker_folder(&st.current_folder)),
    );

    let list = GetDlgItem(hwnd, IDC_PICKER_LIST as i32);
    SendMessageW(list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    if let Some(error) = &result.error {
        msgbox(hwnd, error, "Remote Folder");
        return LRESULT(0);
    }

    for entry in &result.entries {
        let label = display_folder_name(entry);
        let text = hstring(&label);
        let idx = SendMessageW(
            list,
            LB_ADDSTRING,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
        )
        .0;
        if idx >= 0 {
            let stored = Box::new(entry.clone());
            SendMessageW(
                list,
                LB_SETITEMDATA,
                WPARAM(idx as usize),
                LPARAM(Box::into_raw(stored) as isize),
            );
        }
    }

    EnableWindow(
        GetDlgItem(hwnd, IDC_PICKER_UP as i32),
        BOOL(!st.current_folder.is_empty() as i32),
    );
    EnableWindow(GetDlgItem(hwnd, IDC_PICKER_SELECT as i32), TRUE);
    LRESULT(0)
}

unsafe fn picker_on_destroy(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
    if ptr.is_null() {
        return;
    }

    let list = GetDlgItem(hwnd, IDC_PICKER_LIST as i32);
    let count = SendMessageW(list, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0;
    for idx in 0..count.max(0) as usize {
        let data = SendMessageW(list, LB_GETITEMDATA, WPARAM(idx), LPARAM(0)).0;
        if data >= 0 {
            let _ = Box::from_raw(data as *mut String);
        }
    }

    let st = Box::from_raw(ptr);
    DeleteObject(st.hfont);
    DeleteObject(st.hfont_b);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
}

unsafe fn picker_load_current(hwnd: HWND) {
    let st = picker_state(hwnd);
    if st.busy {
        return;
    }

    st.busy = true;
    let cfg = st.cfg.clone();
    let password = st.password.clone();
    let folder = st.current_folder.clone();
    let raw = hwnd.0 as isize;
    let _ = SetWindowTextW(
        GetDlgItem(hwnd, IDC_PICKER_PATH as i32),
        &hstring(&display_picker_folder(&folder)),
    );
    EnableWindow(GetDlgItem(hwnd, IDC_PICKER_SELECT as i32), FALSE);

    std::thread::spawn(move || {
        let mut resolved_folder = folder.clone();
        let load = loop {
            let url = join_remote_url(&cfg.webdav_url, &resolved_folder);
            match webdav::list_folders(&cfg, &password, &url) {
                Ok(entries) => {
                    break PickerLoadResult {
                        entries: entries
                            .into_iter()
                            .map(|href| relative_folder_from_href(&cfg.webdav_url, &href))
                            .filter(|p| !p.is_empty())
                            .collect(),
                        error: None,
                        resolved_folder: resolved_folder.clone(),
                    }
                }
                Err(error) => {
                    if resolved_folder.is_empty() {
                        break PickerLoadResult {
                            entries: Vec::new(),
                            error: Some(error),
                            resolved_folder: String::new(),
                        };
                    }
                    resolved_folder = parent_folder(&resolved_folder);
                }
            }
        };

        let boxed = Box::new(load);
        unsafe {
            PostMessageW(
                HWND(raw),
                WM_APP_PICKER_LOADED,
                WPARAM(0),
                LPARAM(Box::into_raw(boxed) as isize),
            )
            .ok();
        }
    });
}

unsafe fn picker_go_up(hwnd: HWND) {
    let st = picker_state(hwnd);
    st.current_folder = parent_folder(&st.current_folder);
    st.selected_folder = Some(st.current_folder.clone());
    picker_load_current(hwnd);
}

unsafe fn picker_select_current_list_item(hwnd: HWND) {
    let st = picker_state(hwnd);
    let list = GetDlgItem(hwnd, IDC_PICKER_LIST as i32);
    let idx = SendMessageW(list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    if idx < 0 {
        return;
    }

    let idx = idx as usize;
    if let Some(folder) = picker_list_entries(hwnd).get(idx) {
        st.selected_folder = Some(folder.clone());
        let _ = SetWindowTextW(GetDlgItem(hwnd, IDC_PICKER_PATH as i32), &hstring(folder));
    }
}

unsafe fn picker_enter_current_list_item(hwnd: HWND) {
    let st = picker_state(hwnd);
    let list = GetDlgItem(hwnd, IDC_PICKER_LIST as i32);
    let idx = SendMessageW(list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    if idx < 0 {
        return;
    }

    let idx = idx as usize;
    if let Some(folder) = picker_list_entries(hwnd).get(idx) {
        st.current_folder = folder.clone();
        st.selected_folder = Some(folder.clone());
        picker_load_current(hwnd);
    }
}

unsafe fn picker_commit(hwnd: HWND) {
    let st = picker_state(hwnd);
    let chosen = st
        .selected_folder
        .clone()
        .unwrap_or_else(|| st.current_folder.clone());

    (*st.result).folder = Some(normalize_remote_folder(&chosen));
    DestroyWindow(hwnd).ok();
}

unsafe fn picker_state(hwnd: HWND) -> &'static mut PickerState {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState)
}

fn join_remote_url(base: &str, folder: &str) -> String {
    let mut url = base.trim_end_matches('/').to_string();
    let folder = normalize_remote_folder(folder);
    if !folder.is_empty() {
        url.push('/');
        url.push_str(&folder);
    }
    url.push('/');
    url
}

fn normalize_remote_folder(folder: &str) -> String {
    folder
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn parent_folder(folder: &str) -> String {
    let normalized = normalize_remote_folder(folder);
    let mut parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    parts.pop();
    parts.join("/")
}

fn relative_folder_from_href(base_url: &str, href: &str) -> String {
    let href = href.trim();
    let href = href.trim_end_matches('/');
    let base = base_url.trim_end_matches('/');
    let relative = if let Some(rest) = href.strip_prefix(base) {
        rest
    } else {
        href
    };
    normalize_remote_folder(relative)
}

fn display_folder_name(folder: &str) -> String {
    folder
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("/")
        .to_string()
}

fn display_picker_folder(folder: &str) -> String {
    let folder = normalize_remote_folder(folder);
    if folder.is_empty() {
        "/".to_string()
    } else {
        format!("/{folder}")
    }
}

unsafe fn picker_list_entries(hwnd: HWND) -> Vec<String> {
    let list = GetDlgItem(hwnd, IDC_PICKER_LIST as i32);
    let count = SendMessageW(list, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0;
    let mut entries = Vec::new();
    if count <= 0 {
        return entries;
    }

    for idx in 0..count {
        let data = SendMessageW(list, LB_GETITEMDATA, WPARAM(idx as usize), LPARAM(0)).0;
        if data >= 0 {
            entries.push((*(data as *mut String)).clone());
        }
    }
    entries
}

unsafe fn apply_startup(cfg: &Config) {
    use windows::Win32::System::Registry::*;
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut hk = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, key, 0, KEY_SET_VALUE, &mut hk).is_ok() {
        if cfg.start_with_windows {
            if let Ok(exe) = std::env::current_exe() {
                let command = format!("\"{}\"", exe.to_string_lossy());
                let v: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
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
