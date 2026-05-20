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
use qrcodegen::{QrCode, QrCodeEcc};
use std::ffi::c_void;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi as gdi;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging as wam;
use windows::Win32::UI::Shell::{
    DefSubclassProc, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW,
    SetWindowSubclass,
    BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

trait IntoOptionalHwnd {
    fn into_optional_hwnd(self) -> Option<HWND>;
}

impl IntoOptionalHwnd for HWND {
    fn into_optional_hwnd(self) -> Option<HWND> {
        Some(self)
    }
}

impl IntoOptionalHwnd for Option<HWND> {
    fn into_optional_hwnd(self) -> Option<HWND> {
        self
    }
}

trait IntoOptionalHmenu {
    fn into_optional_hmenu(self) -> Option<HMENU>;
}

impl IntoOptionalHmenu for HMENU {
    fn into_optional_hmenu(self) -> Option<HMENU> {
        Some(self)
    }
}

impl IntoOptionalHmenu for Option<HMENU> {
    fn into_optional_hmenu(self) -> Option<HMENU> {
        self
    }
}

trait IntoOptionalHinstance {
    fn into_optional_hinstance(self) -> Option<HINSTANCE>;
}

impl IntoOptionalHinstance for HINSTANCE {
    fn into_optional_hinstance(self) -> Option<HINSTANCE> {
        Some(self)
    }
}

impl IntoOptionalHinstance for Option<HINSTANCE> {
    fn into_optional_hinstance(self) -> Option<HINSTANCE> {
        self
    }
}

#[allow(non_snake_case)]
unsafe fn CreateWindowExW<P1, P2, P7, P8>(
    ex_style: WINDOW_EX_STYLE,
    class_name: P1,
    window_name: P2,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: P7,
    menu: P8,
    instance: impl IntoOptionalHinstance,
    param: Option<*const c_void>,
) -> HWND
where
    P1: Param<PCWSTR>,
    P2: Param<PCWSTR>,
    P7: IntoOptionalHwnd,
    P8: IntoOptionalHmenu,
{
    wam::CreateWindowExW(
        ex_style,
        class_name,
        window_name,
        style,
        x,
        y,
        width,
        height,
        parent.into_optional_hwnd(),
        menu.into_optional_hmenu(),
        instance.into_optional_hinstance(),
        param,
    )
    .unwrap_or_default()
}

#[allow(non_snake_case)]
unsafe fn GetDlgItem(hwnd: HWND, id: i32) -> HWND {
    wam::GetDlgItem(Some(hwnd), id).unwrap_or_default()
}

#[allow(non_snake_case)]
unsafe fn GetParent(hwnd: HWND) -> HWND {
    wam::GetParent(hwnd).unwrap_or_default()
}

#[allow(non_snake_case)]
unsafe fn IsWindow(hwnd: HWND) -> BOOL {
    wam::IsWindow(Some(hwnd))
}

#[allow(non_snake_case)]
unsafe fn LoadIconW<P1>(instance: impl IntoOptionalHinstance, icon_name: P1) -> Result<HICON>
where
    P1: Param<PCWSTR>,
{
    wam::LoadIconW(instance.into_optional_hinstance(), icon_name)
}

#[allow(non_snake_case)]
unsafe fn InvalidateRect(hwnd: HWND, rect: Option<&RECT>, erase: BOOL) -> BOOL {
    gdi::InvalidateRect(
        Some(hwnd),
        rect.map(|rc| rc as *const RECT),
        erase.as_bool(),
    )
}

#[allow(non_snake_case)]
unsafe fn GetWindowDC(hwnd: HWND) -> HDC {
    gdi::GetWindowDC(Some(hwnd))
}

#[allow(non_snake_case)]
unsafe fn ReleaseDC(hwnd: impl IntoOptionalHwnd, hdc: HDC) -> i32 {
    gdi::ReleaseDC(hwnd.into_optional_hwnd(), hdc)
}

#[allow(non_snake_case)]
unsafe fn DeleteObject(object: impl Into<HGDIOBJ>) -> BOOL {
    gdi::DeleteObject(object.into())
}

#[allow(non_snake_case)]
unsafe fn SelectObject(hdc: HDC, object: impl Into<HGDIOBJ>) -> HGDIOBJ {
    gdi::SelectObject(hdc, object.into())
}

#[allow(non_snake_case)]
unsafe fn DrawIconEx(
    hdc: HDC,
    x: i32,
    y: i32,
    icon: HICON,
    width: i32,
    height: i32,
    step: u32,
    brush: HBRUSH,
    flags: DI_FLAGS,
) -> BOOL {
    BOOL::from(
        wam::DrawIconEx(
            hdc,
            x,
            y,
            icon,
            width,
            height,
            step,
            (!brush.0.is_null()).then_some(brush),
            flags,
        )
        .is_ok(),
    )
}

#[allow(non_snake_case)]
unsafe fn SendMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    wam::SendMessageW(hwnd, msg, Some(wparam), Some(lparam))
}

#[allow(non_snake_case)]
unsafe fn PostMessageW(
    hwnd: impl IntoOptionalHwnd,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<()> {
    wam::PostMessageW(hwnd.into_optional_hwnd(), msg, wparam, lparam)
}

#[allow(non_snake_case)]
unsafe fn SetTimer(hwnd: HWND, timer_id: usize, elapsed_ms: u32, proc: TIMERPROC) -> usize {
    wam::SetTimer(Some(hwnd), timer_id, elapsed_ms, proc)
}

#[allow(non_snake_case)]
unsafe fn KillTimer(hwnd: HWND, timer_id: usize) -> Result<()> {
    wam::KillTimer(Some(hwnd), timer_id)
}

#[allow(non_snake_case)]
unsafe fn GetDeviceCaps(hdc: HDC, index: GET_DEVICE_CAPS_INDEX) -> i32 {
    gdi::GetDeviceCaps(Some(hdc), index)
}

#[allow(non_snake_case)]
unsafe fn MessageBoxW<P1, P2>(
    hwnd: HWND,
    text: P1,
    caption: P2,
    style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT
where
    P1: Param<PCWSTR>,
    P2: Param<PCWSTR>,
{
    wam::MessageBoxW(Some(hwnd), text, caption, style)
}

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
const C_AMBER: u32 = 0x0000A5FF; // waiting / pending approval
const C_RED: u32 = 0x000000CC; // not connected
const C_DIVIDER: u32 = 0x00E0E0E0; // section separator line

// ── Control IDs ──────────────────────────────────────────────────────────────
const IDC_WATCH_FOLDER: u16 = 101;
const IDC_BROWSE_LOCAL: u16 = 102;
const IDC_OPEN_LOCAL_FOLDER: u16 = 124;
const IDC_URL: u16 = 103;
const IDC_USERNAME: u16 = 104;
const IDC_PASSWORD: u16 = 105;
const IDC_REMOTE_FOLDER: u16 = 106;
const IDC_CONNECT: u16 = 108;
const IDC_SERVER_STATUS: u16 = 123;
const IDC_SYNC_STATUS: u16 = 117;
const IDC_ACTIVITY_LIST: u16 = 114;
const IDC_START_WINDOWS: u16 = 115;
const IDC_SYNC_REMOTE: u16 = 116;
const IDC_SYNC_PROGRESS: u16 = 118;
const IDC_REPO: u16 = 120;
const IDC_DEST_CREATED: u16 = 121;
const IDC_UPDATE_LINK: u16 = 122;
const IDC_SERVER_HDR: u16 = 207;
const IDC_GITHUB: u16 = 211;
const IDC_AUTHOR: u16 = 212;
const IDC_ORIGIN_LABEL: u16 = 213;
const IDC_DEST_LABEL: u16 = 214;
const IDC_ACTIVITY_HDR: u16 = 215;
const IDC_PAIR_DEVICE: u16 = 217;
const IDC_SERVER_URL_LABEL: u16 = 218;
const IDC_SYNC_ETA: u16 = 219;
const IDC_RETRY_FAILED: u16 = 220;

const WM_APP_LOG: u32 = WM_APP + 10;
const WM_APP_CONNECTED: u32 = WM_APP + 11;
const WM_APP_UPDATE: u32 = WM_APP + 12;
const WM_APP_REMOTE_FOLDER: u32 = WM_APP + 13;
const WM_APP_SYNC_ACTIVITY: u32 = WM_APP + 16;
const WM_APP_PAIR_RESULT: u32 = WM_APP + 17;
const WM_APP_PAIR_STARTED: u32 = WM_APP + 18;
const WM_APP_AUTH_FAILED: u32 = WM_APP + 19;
const IDT_SYNC_ANIM: usize = 1;
const SYNC_ANIM_MS: u32 = 120;

const SS_LEFT: u32 = 0x0000;
const SS_CENTER: u32 = 0x0001;
const SS_RIGHT: u32 = 0x0002;
const SS_NOTIFY: u32 = 0x0100;
pub const CLASS_NAME: PCWSTR = w!("BackupSyncToolWnd");
const REPO_URL: &str = "https://github.com/ruibeard/backup-sync-tool";
const AUTHOR_URL: &str = "https://ruialmeida.me";
const PAIR_QR_CLASS_NAME: PCWSTR = w!("BackupSyncToolPairQrWnd");
const PAIR_QR_CLIENT_W: i32 = 380;
const PAIR_QR_CLIENT_H: i32 = 500;
const IDC_PAIR_QR_TITLE: u16 = 300;
const IDC_PAIR_QR_STATUS: u16 = 301;
const IDC_PAIR_QR_CODE: u16 = 302;
const IDC_PAIR_QR_LINK: u16 = 303;
const IDC_PAIR_QR_CANCEL: u16 = 304;

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
const ACTION_BTN_W: i32 = 76; // Open / Browse / Connect / Reconnect / footer buttons
const ACTION_BTN_H: i32 = INP_H;
const GITHUB_BTN_SIZE: i32 = ACTION_BTN_H; // square icon hit target in footer
const FOLDER_ACTIONS_W: i32 = ACTION_BTN_W * 2 + PAD;
const CONTENT_TOP_PAD: i32 = 14; // mockup .body padding above status strip
const STATUS_STRIP_H: i32 = 38;
const STATUS_ACCENT_W: i32 = 4;
const DEST_PATH_H: i32 = 30;
const SYNC_FOOTER_H: i32 = 44;
const C_STATUS_BG: u32 = 0x00FFFFFF;
const C_DEST_PATH_BG: u32 = C_FOOTER_IDLE_BG;
const C_DEST_PATH_BORDER: u32 = C_FOOTER_IDLE_BORDER;
const C_PANEL_BORDER: u32 = 0x00CCCCCC;
const C_FOOTER_IDLE_BG: u32 = 0x00FAFAFA;
const C_FOOTER_IDLE_BORDER: u32 = 0x00E0E0E0;
const C_FOOTER_BUSY_BORDER: u32 = 0x00F5D9C5;
const C_STATUS_MUTED: u32 = 0x00888888;
const C_PROGRESS_TRACK: u32 = 0x00E0E0E0;

const MIN_ACTIVITY_LIST_H: i32 = 96;
const INNER_W: i32 = WIN_W - M * 2; // usable inner width
const MAX_ACTIVITY_ROWS: usize = 200;
const ACTIVITY_ROW_H_DONE: i32 = 22;
const ACTIVITY_ROW_H_ACTIVE: i32 = 32;
const ACTIVITY_ROW_H_ERROR: i32 = 36;
const ACTIVITY_PAD_LEFT: i32 = 8;
const ACTIVITY_PAD_RIGHT: i32 = 8;
const ACTIVITY_STATUS_W: i32 = 36;
const C_PROGRESS_MINI: u32 = 0x00FFA500; // #00A5FF BGR
const C_ACTIVITY_TRACK: u32 = 0x00E8E8E8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityKind {
    Info,
    Uploading,
    Downloading,
    Done,
    Error,
}

#[derive(Clone)]
struct ActivityRow {
    label: String,
    kind: ActivityKind,
    pct: Option<u8>,
    /// Short error detail for failed uploads.
    detail: Option<String>,
    /// Relative path under watch_folder (for retry).
    relative_path: Option<String>,
    /// Match key for replacing in-flight rows (e.g. "upload:invoice.pdf").
    replace_key: Option<String>,
}

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
    sync_last_failed: usize,
    sync_started_at: Option<std::time::Instant>,
    sync_anim_frame: usize,
    sync_icon: HICON,
    sync_icon_rect: RECT,
    remote_folder_from_xd: bool,
    detected_customer: Option<String>,
    server_tooltip: HWND,
    server_tooltip_text: Vec<u16>,
    status_dot_color: u32,
    server_status_rect: RECT,
    status_strip_rect: RECT,
    status_strip_display: String,
    /// Optional second segment on the status strip (usually empty; connection-only strip).
    status_strip_secondary: String,
    activity_list_rect: RECT,
    dest_path_rect: RECT,
    sync_footer_rect: RECT,
    sync_footer_busy: bool,
    hfont: HFONT,
    hfont_hdr: HFONT,
    hfont_b: HFONT,
    hfont_small: HFONT,
    hfont_activity: HFONT,
    hfont_btn: HFONT,
    hfont_link: HFONT,
    br_win: HBRUSH,
    br_status_strip: HBRUSH,
    br_path_box: HBRUSH,
    br_footer_idle: HBRUSH,
    br_footer_busy: HBRUSH,
    br_sect: HBRUSH,
    br_input: HBRUSH,
    focused_edit: u16,
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
    pair_qr_hwnd: HWND,
    pair_cancel: Option<Arc<AtomicBool>>,
    pair_id: u64,
    auth_failure_notified: bool,
    activity_rows: Vec<ActivityRow>,
    activity_show_empty: bool,
    /// Relative paths that failed in the last batch(es); cleared on successful upload.
    failed_upload_paths: Vec<String>,
}

struct PairResult {
    pair_id: u64,
    device_token: String,
    webdav_url: String,
    username: String,
    password: String,
    remote_folder: String,
    credential_profile_id: Option<u64>,
    credential_version: Option<u64>,
}

struct PairStarted {
    pair_id: u64,
    code: String,
    approve_url: String,
}

struct PairError {
    pair_id: u64,
    message: String,
}

struct PairQrState {
    parent: HWND,
    code: String,
    approve_url: String,
    ready: bool,
    hfont: HFONT,
    hfont_b: HFONT,
    hfont_code: HFONT,
}
