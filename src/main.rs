// main.rs — entry point
// #![windows_subsystem = "windows"] suppresses the console window on release builds.
#![windows_subsystem = "windows"]

mod config;
mod secret;
mod sync;
mod tray;
mod ui;
mod updater;
mod webdav;
mod xd;

use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinstance = GetModuleHandleW(None).unwrap().into();
        ui::run(hinstance);
    }
}
