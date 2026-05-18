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
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::{
    DefSubclassProc, ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass,
    ShellExecuteW, BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
    BROWSEINFOW,
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
const C_AMBER: u32 = 0x0000A5FF; // waiting / pending approval
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
const IDC_SERVER_HDR: u16 = 207;
const IDC_GITHUB: u16 = 211;
const IDC_AUTHOR: u16 = 212;
const IDC_ORIGIN_LABEL: u16 = 213;
const IDC_DEST_LABEL: u16 = 214;
const IDC_ACTIVITY_HDR: u16 = 215;
const IDC_PAIR_DEVICE: u16 = 217;

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
const SMALL_BTN_H: i32 = 24; // compact secondary button height
const HDR_H: i32 = 20; // section heading height
const LBL_H: i32 = 18; // label text height
const BROWSE_W: i32 = 34; // folder icon button width
const PAIR_BTN_W: i32 = 82;
const SERVER_STATUS_W: i32 = 170;
const MIN_ACTIVITY_LIST_H: i32 = 96;
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
    detected_customer: Option<String>,
    server_tooltip: HWND,
    server_tooltip_text: Vec<u16>,
    status_dot_color: u32,
    hfont: HFONT,
    hfont_hdr: HFONT,
    hfont_b: HFONT,
    hfont_small: HFONT,
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
    pair_qr_hwnd: HWND,
    pair_cancel: Option<Arc<AtomicBool>>,
    pair_id: u64,
    auth_failure_notified: bool,
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
    hfont: HFONT,
    hfont_b: HFONT,
    hfont_code: HFONT,
}

