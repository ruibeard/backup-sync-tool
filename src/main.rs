// main.rs — entry point
// #![windows_subsystem = "windows"] suppresses the console window on release builds.
#![cfg_attr(windows, windows_subsystem = "windows")]
// Platform-specific native shells intentionally leave some shared supervisor
// and pairing helpers unused on the opposite target.
#![cfg_attr(not(windows), allow(dead_code))]

mod app;
mod config;
mod logs;
mod pairing;
mod paths;
mod secret;
mod syncthing;
mod updater;

#[cfg(target_os = "macos")]
mod host;

#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod xd;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
fn main() {
    use windows::core::w;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, MUTEX_ALL_ACCESS};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    // Manual launches show the full status window. Only an explicit startup
    // launch requests background mode; `--show` remains accepted implicitly.
    let start_minimized = std::env::args().any(|arg| arg == "--background");
    unsafe {
        if OpenMutexW(MUTEX_ALL_ACCESS, false, w!("BackupSyncToolSingleton")).is_ok() {
            let hwnd = FindWindowW(ui::CLASS_NAME, None).unwrap_or_default();
            if !hwnd.0.is_null() && !start_minimized {
                ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return;
        }
        let instance_mutex = CreateMutexW(None, true, w!("BackupSyncToolSingleton"));

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinstance = GetModuleHandleW(None).unwrap().into();
        ui::run(hinstance, start_minimized);
        drop(instance_mutex);
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!(
        "backupsynctool: unsupported platform ({}). Windows and macOS only.",
        std::env::consts::OS
    );
    std::process::exit(1);
}
