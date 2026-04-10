// tray.rs — Win32 system tray icon with context menu
// Uses Shell_NotifyIcon + a hidden message-only window to receive tray messages.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 1;
pub const ID_TRAY_OPEN: usize = 1001;
pub const ID_TRAY_LOGS: usize = 1002;
pub const ID_TRAY_EXIT: usize = 1003;

unsafe fn fill_tip(dst: &mut [u16; 128], text: &str) {
    let wide = HSTRING::from(text);
    let src = wide.as_wide();
    let len = src.len().min(127);
    dst[..len].copy_from_slice(&src[..len]);
}

// Add tray icon to the notification area
pub unsafe fn add_tray_icon(hwnd: HWND, hicon: HICON) {
    let mut tip = [0u16; 128];
    fill_tip(&mut tip, "Backup Sync Tool");

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: hicon,
        szTip: tip,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_ADD, &mut nid);
}

pub unsafe fn set_tray_icon_and_tip(hwnd: HWND, hicon: HICON, text: &str) {
    let mut tip = [0u16; 128];
    fill_tip(&mut tip, text);

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_TIP,
        hIcon: hicon,
        szTip: tip,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_MODIFY, &mut nid);
}

// Remove tray icon (call on exit)
pub unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
}

// Show right-click context menu at cursor
pub unsafe fn show_tray_menu(hwnd: HWND) {
    let hmenu = CreatePopupMenu().unwrap();
    AppendMenuW(hmenu, MF_STRING, ID_TRAY_OPEN, w!("Open")).ok();
    AppendMenuW(hmenu, MF_STRING, ID_TRAY_LOGS, w!("Open Logs")).ok();
    AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
    AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT, w!("Exit")).ok();

    let mut pt = windows::Win32::Foundation::POINT::default();
    windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt).ok();

    // Required so menu dismisses when clicking elsewhere
    let _ = SetForegroundWindow(hwnd).ok();

    TrackPopupMenu(hmenu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    DestroyMenu(hmenu).ok();
}
