unsafe fn apply_startup(cfg: &Config) {
    use windows::Win32::System::Registry::*;
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut hk = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, key, Some(0), KEY_SET_VALUE, &mut hk).is_ok() {
        if cfg.start_with_windows {
            if let Ok(exe) = std::env::current_exe() {
                let command = format!("\"{}\" --background", exe.to_string_lossy());
                let v: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = RegSetValueExW(
                    hk,
                    w!("BackupSyncTool"),
                    Some(0),
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
