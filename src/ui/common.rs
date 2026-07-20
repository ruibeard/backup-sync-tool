// ui.rs — Win32 main window
//
// Design (mockup target):
//   Window bg:       #FFFFFF
//   No card boxes — sections separated by spacing + section headings only
//   Section headings: #888888, Segoe UI 10pt SemiBold, ALL CAPS
//   Field labels:     above inputs, left-aligned
//   Inputs:          white bg, 1px #CCCCCC border (blue on focus)
//   Password field:  eye icon drawn inside right padding of edit subclass
//   Connect/Save:    blue #2B4FA3, white text; Save is primary, Close secondary
//   Choose/Close:    #E8E8E8 grey, #333333 text
//   Status dot:      inline on the SERVER heading row
//   Bottom bar:      version + checkboxes on one row; SAVE right
//   Spacing:         PAD=8, GAP=12, SECT=20 rhythm
//
// Typography (Segoe UI, pixel heights — readable desktop scale):
//   13px normal   — body: activity rows, checkboxes, footer buttons
//   12px underline — footer links (version, author)
//   11px bold     — section headings (RECENT ACTIVITY)
//   12px semibold — status pill, bridge node names, bridge mid ETA
//   12px normal   — captions/subtitles, bridge paths, activity status
//   18px semibold — bridge mid checkmark (icon-like)
const FONT_BODY_PX: i32 = 14;
const FONT_CAPTION_PX: i32 = 13;
const FONT_SECTION_PX: i32 = 12;
const FONT_EMPHASIS_PX: i32 = 15;
const FONT_BTN_PX: i32 = 14;
const FONT_BTN_SM_PX: i32 = 14;
const FONT_LINK_PX: i32 = 13;
const FONT_BRIDGE_CHECK_PX: i32 = 16;

use crate::config::{self, Config};
use crate::logs;
use crate::tray;
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
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass,
    BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging as wam;
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
const C_WIN_BG: u32 = 0x00FFFFFF;
const C_LABEL: u32 = 0x00554833;
const C_INPUT_BG: u32 = 0x00FFFFFF;
const C_INPUT_BORDER: u32 = 0x00E6DDD7;
const C_INPUT_FOCUS: u32 = 0x00EB6325;
const C_BLUE: u32 = 0x00EB6325;
const C_BLUE_HOV: u32 = 0x00D1581D;
const C_BLUE_TXT: u32 = 0x00FFFFFF;
const C_GREY_BTN: u32 = 0x00FCFAF8;
const C_GREY_HOV: u32 = 0x00F4F0EA;
const C_GREY_TXT: u32 = C_LABEL;
const C_GREY_BORDER: u32 = 0x00D6D0C8;
const C_GREEN: u32 = 0x003F7A16; // connected
const C_AMBER: u32 = 0x0000A5FF; // waiting / pending approval
const C_RED: u32 = 0x000000CC; // not connected
const C_DIVIDER: u32 = 0x00F0ECE6; // section separator line

// ── Control IDs ──────────────────────────────────────────────────────────────
const IDC_WATCH_FOLDER: u16 = 101;
const IDC_BROWSE_LOCAL: u16 = 102;
const IDC_OPEN_LOCAL_FOLDER: u16 = 124;
const IDC_REMOTE_FOLDER: u16 = 106;
const IDC_SERVER_STATUS: u16 = 123;
const IDC_SYNC_STATUS: u16 = 117;
const IDC_ACTIVITY_LIST: u16 = 114;
const IDC_START_WINDOWS: u16 = 115;
const IDC_AUTO_UPDATE: u16 = 119;
const IDC_SYNC_PROGRESS: u16 = 118;
const IDC_REPO: u16 = 120;
const IDC_UPDATE_LINK: u16 = 122;
const IDC_SERVER_HDR: u16 = 207;
const IDC_GITHUB: u16 = 211;
const IDC_AUTHOR: u16 = 212;
const IDC_ORIGIN_LABEL: u16 = 213;
const IDC_DEST_LABEL: u16 = 214;
const IDC_ACTIVITY_HDR: u16 = 215;
const IDC_ACTIVITY_SUBHDR: u16 = 216;
const IDC_PAIR_DEVICE: u16 = 217;
const IDC_SERVER_URL_LABEL: u16 = 218;
const IDC_SYNC_ETA: u16 = 219;
const IDC_REFRESH_REMOTE: u16 = 221;
const IDC_PAIR_API_BASE: u16 = 222;
const IDC_PAIR_API_LABEL: u16 = 223;
const IDC_SERVER_DELETION_POLICY: u16 = 224;

const WM_APP_LOG: u32 = WM_APP + 10;
const WM_APP_CONNECTED: u32 = WM_APP + 11;
const WM_APP_UPDATE: u32 = WM_APP + 12;
const WM_APP_REMOTE_FOLDER: u32 = WM_APP + 13;
const WM_APP_PAIR_RESULT: u32 = WM_APP + 17;
const WM_APP_PAIR_STARTED: u32 = WM_APP + 18;
const WM_APP_APP_SNAPSHOT: u32 = WM_APP + 22;
const WM_APP_REPAIR_FAILED: u32 = WM_APP + 23;
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
const PAIR_QR_CLIENT_H: i32 = 524;
const IDC_PAIR_QR_TITLE: u16 = 300;
const IDC_PAIR_QR_STATUS: u16 = 301;
const IDC_PAIR_QR_CODE: u16 = 302;
const IDC_PAIR_QR_LINK: u16 = 303;
const IDC_PAIR_QR_CANCEL: u16 = 304;
const IDC_PAIR_QR_SERVER: u16 = 305;
const IDC_PAIR_QR_CHANGE_SERVER: u16 = 306;
const IDC_PAIR_QR_SERVER_EDIT: u16 = 307;
const IDC_PAIR_QR_EXPIRY: u16 = 308;

// ── Layout — 8/12/20 rhythm ──────────────────────────────────────────────────
const WIN_W: i32 = 520;
const M: i32 = 24;
const PAD: i32 = 10;
const GAP: i32 = 14;
const SECT: i32 = 22;
const INP_H: i32 = 30; // input height
const BTN_H: i32 = 32; // bottom-bar primary button height
const HDR_H: i32 = 22; // section heading height
const LBL_H: i32 = 20; // label text height
const ACTION_BTN_W: i32 = 94; // Open / Choose / Connect / Reconnect / footer buttons
const ACTION_BTN_H: i32 = INP_H;
const GITHUB_BTN_SIZE: i32 = ACTION_BTN_H; // square icon hit target in footer
const META_ICON_GAP: i32 = 5; // gap between version link and GitHub icon
const CONTENT_TOP_PAD: i32 = 20;
const BRIDGE_PAD_Y: i32 = 0;
const BRIDGE_NODE_GAP: i32 = 24;
const BRIDGE_ICO: i32 = 40;
const BRIDGE_ICO_TILE: i32 = 52;
const BRIDGE_BTN_H: i32 = 30;
const BRIDGE_HEADER_H: i32 = BRIDGE_ICO_TILE + 5 + BRIDGE_NAME_H + 2 + BRIDGE_PATH_H;
const BRIDGE_OPEN_BTN_W: i32 = 58;
const BRIDGE_BROWSE_BTN_W: i32 = 82;
const BRIDGE_PAIR_BTN_W: i32 = 150;
const BRIDGE_DIVIDER_VPAD: i32 = 12;
const BRIDGE_NAME_H: i32 = 20;
const BRIDGE_CONN_LABEL_H: i32 = 18;
const SYNC_BAR_H: i32 = 10;
const SYNC_BAND_HEAD_H: i32 = 20;
const SYNC_BAND_HEAD_BAR_GAP: i32 = 6;
const SYNC_BAND_BAR_DETAIL_GAP: i32 = 4;
const SYNC_BAND_DETAIL_H: i32 = 18;
const SYNC_BAND_H: i32 = SYNC_BAND_HEAD_H
    + SYNC_BAND_HEAD_BAR_GAP
    + SYNC_BAR_H
    + SYNC_BAND_BAR_DETAIL_GAP
    + SYNC_BAND_DETAIL_H;
const BRIDGE_PATH_H: i32 = 18;
const BRIDGE_CONTENT_H: i32 =
    BRIDGE_HEADER_H + BRIDGE_DIVIDER_VPAD + BRIDGE_BTN_H + BRIDGE_DIVIDER_VPAD + 1;
const BRIDGE_H: i32 = BRIDGE_PAD_Y + BRIDGE_CONTENT_H + BRIDGE_PAD_Y;
const STATUS_ROW_H: i32 = 0;

struct BridgeLayout {
    height: i32,
    divider_y: i32,
    btn_y: i32,
    open_btn_x: i32,
    open_btn_y: i32,
    open_btn_w: i32,
    browse_btn_x: i32,
    browse_btn_w: i32,
    pair_btn_x: i32,
    pair_btn_w: i32,
    left_tile: RECT,
    right_tile: RECT,
    left_ico: RECT,
    right_ico: RECT,
    right_name: RECT,
    left_path: RECT,
    right_conn: RECT,
}

fn bridge_layout_at(top: i32, inner_w: i32) -> BridgeLayout {
    let header_top = top + BRIDGE_PAD_Y;
    let mid = inner_w / 2;
    let node_w = (inner_w - BRIDGE_NODE_GAP) / 2;
    let left_node_x = 0;
    let right_node_x = inner_w - node_w;
    let tile_x = |node_x: i32| node_x + (node_w - BRIDGE_ICO_TILE) / 2;

    let left_tile = RECT {
        left: tile_x(left_node_x),
        top: header_top,
        right: tile_x(left_node_x) + BRIDGE_ICO_TILE,
        bottom: header_top + BRIDGE_ICO_TILE,
    };
    let left_ico = RECT {
        left: left_tile.left + (BRIDGE_ICO_TILE - BRIDGE_ICO) / 2,
        top: left_tile.top + (BRIDGE_ICO_TILE - BRIDGE_ICO) / 2,
        right: left_tile.left + (BRIDGE_ICO_TILE + BRIDGE_ICO) / 2,
        bottom: left_tile.top + (BRIDGE_ICO_TILE + BRIDGE_ICO) / 2,
    };
    let name_y = left_tile.bottom + 5;
    let path_y = name_y + BRIDGE_NAME_H + 2;

    let right_tile = RECT {
        left: tile_x(right_node_x),
        top: header_top,
        right: tile_x(right_node_x) + BRIDGE_ICO_TILE,
        bottom: header_top + BRIDGE_ICO_TILE,
    };
    let right_ico = RECT {
        left: right_tile.left + (BRIDGE_ICO_TILE - BRIDGE_ICO) / 2,
        top: right_tile.top + (BRIDGE_ICO_TILE - BRIDGE_ICO) / 2,
        right: right_tile.left + (BRIDGE_ICO_TILE + BRIDGE_ICO) / 2,
        bottom: right_tile.top + (BRIDGE_ICO_TILE + BRIDGE_ICO) / 2,
    };

    let btn_y = header_top + BRIDGE_HEADER_H + BRIDGE_DIVIDER_VPAD;
    let divider_y = btn_y + BRIDGE_BTN_H + BRIDGE_DIVIDER_VPAD;
    let local_btns_w = BRIDGE_OPEN_BTN_W + PAD + BRIDGE_BROWSE_BTN_W;
    BridgeLayout {
        height: BRIDGE_H,
        divider_y,
        btn_y,
        open_btn_x: left_node_x + (node_w - local_btns_w) / 2,
        open_btn_y: btn_y,
        open_btn_w: BRIDGE_OPEN_BTN_W,
        browse_btn_x: left_node_x + (node_w - local_btns_w) / 2 + BRIDGE_OPEN_BTN_W + PAD,
        browse_btn_w: BRIDGE_BROWSE_BTN_W,
        pair_btn_x: right_node_x + (node_w - BRIDGE_PAIR_BTN_W) / 2,
        pair_btn_w: BRIDGE_PAIR_BTN_W,
        left_tile,
        right_tile,
        left_ico,
        right_ico,
        right_name: RECT {
            left: mid + PAD,
            top: name_y,
            right: inner_w,
            bottom: name_y + BRIDGE_NAME_H,
        },
        left_path: RECT {
            left: left_node_x,
            top: name_y,
            right: mid - PAD,
            bottom: name_y + BRIDGE_PATH_H,
        },
        right_conn: RECT {
            left: mid + PAD,
            top: path_y,
            right: inner_w,
            bottom: path_y + BRIDGE_CONN_LABEL_H,
        },
    }
}

const C_BRIDGE_ICO_BG: u32 = 0x00FBF3EE; // #eef3fb
const C_BRIDGE_ICO_BORDER: u32 = 0x00F0DCD0; // #d0dcf0
const C_PILL_GREEN_BG: u32 = 0x00E9F5E8; // #e8f5e9
const C_PILL_SYNC_BG: u32 = 0x00FDF2E3; // #e3f2fd
const C_PILL_SYNC_TXT: u32 = 0x00C06515; // #1565c0
const SYNC_FOOTER_H: i32 = 44;
const C_STATUS_BG: u32 = 0x00FFFFFF;
const C_DEST_PATH_BG: u32 = C_FOOTER_IDLE_BG;
const C_PANEL_BORDER: u32 = 0x00CCCCCC;
const C_FOOTER_IDLE_BG: u32 = 0x00FAFAFA;
const C_FOOTER_IDLE_BORDER: u32 = 0x00E0E0E0;
const C_FOOTER_BUSY_BORDER: u32 = 0x00F5D9C5;
const C_STATUS_MUTED: u32 = 0x008B7464;
const C_BRIDGE_CONN_OK: u32 = C_GREEN;
const C_BRIDGE_CONN_FAIL: u32 = C_RED;
const C_BRIDGE_SYNC_HEAD_OK: u32 = C_GREEN;
const C_BRIDGE_SYNC_HEAD_ACTIVE: u32 = C_PILL_SYNC_TXT;
const C_BRIDGE_SYNC_HEAD_IDLE: u32 = C_STATUS_MUTED;
const C_BRIDGE_PATH_TXT: u32 = 0x008B7464;
const C_PROGRESS_TRACK: u32 = 0x00E0E0E0;

const MIN_ACTIVITY_LIST_H: i32 = 152;
const INNER_W: i32 = WIN_W - M * 2; // usable inner width
const MAX_ACTIVITY_ROWS: usize = 200;
const ACTIVITY_ROW_H_DONE: i32 = 30;
const ACTIVITY_ROW_H_ACTIVE: i32 = 40;
const ACTIVITY_ROW_H_ERROR: i32 = 48;
const ACTIVITY_PAD_LEFT: i32 = 12;
const ACTIVITY_PAD_RIGHT: i32 = 10;
const ACTIVITY_STATUS_W: i32 = 78;
const C_PROGRESS_MINI: u32 = 0x00FFA500; // #00A5FF BGR
const C_ACTIVITY_TRACK: u32 = 0x00E8E8E8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityKind {
    Info,
    Syncing,
    Done,
    Error,
}

#[derive(Clone)]
struct ActivityRow {
    label: String,
    kind: ActivityKind,
    pct: Option<u8>,
    /// Short error detail for an item that needs attention.
    detail: Option<String>,
    /// Match key for replacing an in-flight sync row.
    replace_key: Option<String>,
    /// Local time label for info rows (e.g. "10:42 AM").
    time_label: Option<String>,
}

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum UiSyncState {
    Idle = 0,
    Checking = 1,
    Syncing = 2,
}

// ── Window state ──────────────────────────────────────────────────────────────
struct WndState {
    app: crate::app::AppHandle,
    config: Config,
    sync_engine: Option<crate::sync::SyncEngine>,
    update_url: Option<String>,
    repair_required: bool,
    connected: bool,
    sync_status_text: String,
    sync_status_state: usize,
    sync_progress_done: usize,
    sync_progress_total: usize,
    sync_anim_frame: usize,
    remote_folder_from_xd: bool,
    detected_customer: Option<String>,
    server_tooltip: HWND,
    server_tooltip_text: Vec<u16>,
    status_dot_color: u32,
    server_status_rect: RECT,
    status_strip_rect: RECT,
    status_strip_display: String,
    status_subtitle: String,
    bridge_rect: RECT,
    bridge_progress_rect: RECT,
    bridge_sync_head: String,
    bridge_sync_meta: String,
    bridge_conn_label: String,
    bridge_conn_ok: bool,
    bridge_btn_y: i32,
    bridge_icon_pc: HBITMAP,
    bridge_icon_cloud: HBITMAP,
    inner_w: i32,
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
    hfont_bridge: HFONT,
    hfont_bridge_name: HFONT,
    hfont_bridge_path: HFONT,
    hfont_bridge_mid: HFONT,
    hfont_bridge_check: HFONT,
    hfont_link: HFONT,
    br_win: HBRUSH,
    br_path_box: HBRUSH,
    br_footer_idle: HBRUSH,
    br_footer_busy: HBRUSH,
    br_input: HBRUSH,
    focused_edit: u16,
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
    /// Layout: minimum window client height
    min_client_h: i32,
    /// Grey footer panel behind checkboxes + meta row
    footer_panel_rect: RECT,
    pair_qr_hwnd: HWND,
    pair_cancel: Option<Arc<AtomicBool>>,
    sync_cancel: Option<Arc<AtomicBool>>,
    pair_id: u64,
    auth_failure_notified: bool,
    last_event_id: u64,
    activity_rows: Vec<ActivityRow>,
    activity_show_empty: bool,
}

fn is_paired(cfg: &Config) -> bool {
    config::is_paired(cfg)
}

fn bridge_show_sync_band(st: &WndState) -> bool {
    is_paired(&st.config) && !st.auth_failure_notified
}

fn bridge_syncing_progress(st: &WndState) -> bool {
    st.sync_status_state == UiSyncState::Syncing as usize
        && st.sync_progress_total > 0
}

fn bridge_section_total_h(st: &WndState) -> i32 {
    let band = if bridge_show_sync_band(st) {
        SECT + SYNC_BAND_H
    } else {
        0
    };
    BRIDGE_H + band + GAP
}

fn activity_subhdr_text() -> String {
    format!("This run · up to {MAX_ACTIVITY_ROWS}")
}

struct PairResult {
    pair_id: u64,
    device_uuid: String,
    device_token: String,
    destination_uuid: String,
    destination_label: String,
    chunk_endpoint: String,
    chunk_region: String,
    chunk_bucket: String,
    chunk_prefix: String,
    chunk_access_key: String,
    chunk_secret_key: String,
    chunk_path_style: bool,
}

struct PairStarted {
    pair_id: u64,
    code: String,
    approve_url: String,
}

struct PairError {
    pair_id: u64,
    message: String,
    approval_received: bool,
}

struct PairQrState {
    parent: HWND,
    api_base: String,
    code: String,
    approve_url: String,
    ready: bool,
    editing_server: bool,
    hfont: HFONT,
    hfont_b: HFONT,
    hfont_code: HFONT,
}
