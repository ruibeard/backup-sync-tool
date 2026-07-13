//! Encrypt/decrypt for S3 secret / device token storage.
//! Windows: DPAPI. macOS: Keychain (`kc1:<account>` handle in config JSON).

#[cfg(windows)]
mod dpapi {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    const ENTROPY: &[u8] = b"webdavsync-v1";

    pub fn encrypt(plaintext: &str) -> Result<String, String> {
        if plaintext.is_empty() {
            return Ok(String::new());
        }
        let bytes = plaintext.as_bytes();
        let entropy_copy = ENTROPY.to_vec();

        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: bytes.len() as u32,
                pbData: bytes.as_ptr() as *mut u8,
            };
            let entropy = CRYPT_INTEGER_BLOB {
                cbData: entropy_copy.len() as u32,
                pbData: entropy_copy.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };

            let ok = CryptProtectData(
                &input,
                windows::core::w!("webdavsync"),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );

            if ok.is_ok() {
                let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
                let encoded = B64.encode(slice);
                let _ = LocalFree(Some(HLOCAL(output.pbData as *mut _)));
                Ok(encoded)
            } else {
                Err("CryptProtectData failed".into())
            }
        }
    }

    pub fn decrypt(encoded: &str) -> Result<String, String> {
        if encoded.is_empty() {
            return Ok(String::new());
        }
        let mut cipher = B64.decode(encoded).map_err(|e| e.to_string())?;
        let entropy_copy = ENTROPY.to_vec();

        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: cipher.len() as u32,
                pbData: cipher.as_mut_ptr(),
            };
            let entropy = CRYPT_INTEGER_BLOB {
                cbData: entropy_copy.len() as u32,
                pbData: entropy_copy.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };

            let ok = CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );

            if ok.is_ok() {
                let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
                let plain = String::from_utf8_lossy(slice).into_owned();
                let _ = LocalFree(Some(HLOCAL(output.pbData as *mut _)));
                Ok(plain)
            } else {
                Err("CryptUnprotectData failed".into())
            }
        }
    }
}

#[cfg(windows)]
pub use dpapi::{decrypt, encrypt};

/// Labeled protect (Windows ignores account — same as encrypt).
#[cfg(windows)]
pub fn protect(_account: &str, plaintext: &str) -> Result<String, String> {
    encrypt(plaintext)
}

// --- macOS Keychain -------------------------------------------------------------
//
// Ad-hoc rebuilds change the app CDHash every time. Default Keychain ACLs bind to
// that hash → macOS re-asks for the login password on every launch.
// Store with `security … -A` (allow any app) so local rebuilds stop prompting.
// Tradeoff: any local process that knows service+account can read the secret.

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "cam.rui.backupsynctool";

#[cfg(target_os = "macos")]
const KC_PREFIX: &str = "kc1:";

#[cfg(target_os = "macos")]
fn kc_item_exists(account: &str) -> bool {
    use std::process::Command;
    Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn kc_delete(account: &str) {
    use std::process::Command;
    let _ = Command::new("security")
        .args([
            "delete-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
        ])
        .status();
}

#[cfg(target_os = "macos")]
fn kc_store(account: &str, plaintext: &str) -> Result<String, String> {
    use std::process::Command;
    // Delete first: `-U` can leave old CDHash-bound ACL on existing items.
    kc_delete(account);
    // -A: any app may read without Keychain prompt (needed for ad-hoc rebuilds).
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
            plaintext,
            "-A",
        ])
        .status()
        .map_err(|e| format!("Keychain set failed: {e}"))?;
    if !status.success() {
        return Err(format!("Keychain set failed (exit {status})"));
    }
    Ok(format!("{KC_PREFIX}{account}"))
}

#[cfg(target_os = "macos")]
fn kc_load_account(account: &str) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    if account.is_empty() {
        return Err("empty Keychain account in secret handle".into());
    }

    let mut child = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Keychain get failed: {e}"))?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = child.wait_with_output();
        let _ = tx.send(out);
    });

    let out = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("Keychain get failed: {e}")),
        Err(_) => {
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
            if kc_item_exists(account) {
                kc_delete(account);
            }
            return Err(
                "Keychain get timed out (stale ACL item removed — pair again if sync stops)"
                    .into(),
            );
        }
    };

    if !out.status.success() {
        if kc_item_exists(account) {
            kc_delete(account);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Keychain get failed: {err}"));
    }

    let plain = String::from_utf8(out.stdout)
        .map_err(|e| format!("Keychain secret not UTF-8: {e}"))?
        .trim_end_matches('\n')
        .to_string();

    // Rewrite with -A every successful read so ad-hoc rebuilds stay prompt-free.
    let _ = kc_store(account, &plain);
    Ok(plain)
}

#[cfg(target_os = "macos")]
fn kc_load(encoded: &str) -> Result<String, String> {
    let account = encoded
        .strip_prefix(KC_PREFIX)
        .ok_or_else(|| "invalid Keychain secret handle (expected kc1:…)".to_string())?;
    kc_load_account(account)
}

/// Drop ACL-bound Keychain items that would otherwise prompt on every ad-hoc rebuild.
#[cfg(target_os = "macos")]
pub fn purge_stale_keychain_handles(handles: &[&str]) {
    for encoded in handles {
        let Some(account) = encoded.strip_prefix(KC_PREFIX) else {
            continue;
        };
        if account.is_empty() || !kc_item_exists(account) {
            continue;
        }
        if kc_load_account(account).is_err() {
            kc_delete(account);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn protect(account: &str, plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    if account.is_empty() {
        return Err("Keychain protect requires non-empty account".into());
    }
    kc_store(account, plaintext)
}

#[cfg(target_os = "macos")]
pub fn decrypt(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    kc_load(encoded)
}
