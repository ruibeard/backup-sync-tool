// secret.rs — DPAPI encrypt/decrypt for password storage
// CryptProtectData / CryptUnprotectData bind the blob to the current user + machine.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
// In windows-0.52, LocalFree and HLOCAL both live in Win32::Foundation.
// HLOCAL wraps *mut c_void (not isize).
use windows::Win32::Foundation::{LocalFree, HLOCAL};

const ENTROPY: &[u8] = b"webdavsync-v1";

pub fn encrypt(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let bytes = plaintext.as_bytes();
    let mut entropy_copy = ENTROPY.to_vec();

    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy_copy.len() as u32,
            pbData: entropy_copy.as_mut_ptr(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        // CryptProtectData returns Result<()> in windows-0.52
        let ok = CryptProtectData(
            &input,
            windows::core::w!("webdavsync"),
            Some(&mut entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );

        if ok.is_ok() {
            let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
            let encoded = B64.encode(slice);
            // Free the output blob allocated by Windows
            let _ = LocalFree(HLOCAL(output.pbData as *mut _));
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
    let mut entropy_copy = ENTROPY.to_vec();

    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: cipher.len() as u32,
            pbData: cipher.as_mut_ptr(),
        };
        let mut entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy_copy.len() as u32,
            pbData: entropy_copy.as_mut_ptr(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = CryptUnprotectData(
            &input,
            None,
            Some(&mut entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );

        if ok.is_ok() {
            let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
            let plain = String::from_utf8_lossy(slice).into_owned();
            let _ = LocalFree(HLOCAL(output.pbData as *mut _));
            Ok(plain)
        } else {
            Err("CryptUnprotectData failed".into())
        }
    }
}
