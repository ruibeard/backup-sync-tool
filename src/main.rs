// main.rs — entry point
// #![windows_subsystem = "windows"] suppresses the console window on release builds.
#![windows_subsystem = "windows"]

mod config;
mod logs;
mod secret;
mod sync;
mod tray;
mod ui;
mod updater;
mod webdav;
mod xd;

use windows::core::w;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, MUTEX_ALL_ACCESS};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

fn main() {
    unsafe {
        if OpenMutexW(MUTEX_ALL_ACCESS, false, w!("BackupSyncToolSingleton")).is_ok() {
            let hwnd = FindWindowW(ui::CLASS_NAME, None);
            if hwnd.0 != 0 {
                ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return;
        }
        let instance_mutex = CreateMutexW(None, true, w!("BackupSyncToolSingleton"));

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinstance = GetModuleHandleW(None).unwrap().into();
        ui::run(hinstance);
        drop(instance_mutex);
    }
}
