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

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "cam.rui.backupsynctool";

#[cfg(target_os = "macos")]
const KC_PREFIX: &str = "kc1:";

#[cfg(target_os = "macos")]
fn kc_store(account: &str, plaintext: &str) -> Result<String, String> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(KEYCHAIN_SERVICE, account, plaintext.as_bytes())
        .map_err(|e| format!("Keychain set failed: {e}"))?;
    Ok(format!("{KC_PREFIX}{account}"))
}

#[cfg(target_os = "macos")]
fn kc_load(encoded: &str) -> Result<String, String> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    let account = encoded
        .strip_prefix(KC_PREFIX)
        .ok_or_else(|| "invalid Keychain secret handle (expected kc1:…)".to_string())?;
    if account.is_empty() {
        return Err("empty Keychain account in secret handle".into());
    }
    let bytes = generic_password(PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        account,
    ))
    .map_err(|e| format!("Keychain get failed: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("Keychain secret not UTF-8: {e}"))
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
