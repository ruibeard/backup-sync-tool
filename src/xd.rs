use base64::{engine::general_purpose, Engine as _};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPublicKey};
use serde_json::Value;
use std::fs;
use std::path::Path;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

const XD_ROOT: &str = r"C:\XDSoftware";
const DEFAULT_WATCH_FOLDER: &str = r"C:\XDSoftware\backups";
const XD_LICENSE_PATH: &str = r"C:\XDSoftware\cfg\xd.lic";
const XD_PEM_PATH: &str = r"C:\XDSoftware\cfg\xd.pem";

#[derive(Debug, Clone)]
pub struct DetectedCustomer {
    pub folder: String,
    pub customer: String,
}

pub fn default_watch_folder() -> Option<String> {
    let path = Path::new(DEFAULT_WATCH_FOLDER);
    path.is_dir().then(|| path.display().to_string())
}

pub fn detect_customer_hint() -> Option<DetectedCustomer> {
    if !Path::new(XD_ROOT).is_dir()
        || !Path::new(XD_LICENSE_PATH).is_file()
        || !Path::new(XD_PEM_PATH).is_file()
    {
        return None;
    }

    detect_customer_hint_native().ok()
}

fn detect_customer_hint_native() -> Result<DetectedCustomer, String> {
    let license = fs::read_to_string(XD_LICENSE_PATH).map_err(|err| err.to_string())?;
    let root: Value = serde_json::from_str(&license).map_err(|err| err.to_string())?;
    let pem = fs::read_to_string(XD_PEM_PATH).map_err(|err| err.to_string())?;
    let public_key = read_public_key(&pem)?;

    let number = decrypt_required_json_field(&root, "Number", &public_key)?;
    let customer = decrypt_required_json_field(&root, "ClientComercialName", &public_key)?;
    let folder = build_remote_folder(&number, &customer);
    if folder.is_empty() {
        return Err("Decrypted licence number is empty.".to_string());
    }

    Ok(DetectedCustomer { folder, customer })
}

fn read_public_key(pem: &str) -> Result<RsaPublicKey, String> {
    RsaPublicKey::from_public_key_pem(pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(pem))
        .map_err(|err| err.to_string())
}

fn decrypt_required_json_field(
    root: &Value,
    key: &str,
    public_key: &RsaPublicKey,
) -> Result<String, String> {
    let value = root
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("License JSON field '{key}' is missing."))?;
    Ok(decode_json_field(value, public_key)?.trim().to_string())
}

fn decode_json_field(value: &str, public_key: &RsaPublicKey) -> Result<String, String> {
    if is_encrypted_empty_placeholder(value) {
        return Ok(String::new());
    }

    match try_decrypt_xd_field(value, public_key) {
        Ok(decrypted) if is_mostly_printable(&decrypted) => Ok(decrypted),
        Ok(_) => Err("Decrypted licence value contains control characters.".to_string()),
        Err(_) => Ok(value.to_string()),
    }
}

fn try_decrypt_xd_field(value: &str, public_key: &RsaPublicKey) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("Encrypted licence value is empty.".to_string());
    }

    let mut bytes = Vec::new();
    for part in value.split('=').filter(|part| !part.is_empty()) {
        let block = general_purpose::STANDARD
            .decode(format!("{part}="))
            .map_err(|err| err.to_string())?;
        bytes.extend(raw_rsa_public(&block, public_key));
    }

    String::from_utf8(bytes).map_err(|err| err.to_string())
}

fn raw_rsa_public(block: &[u8], public_key: &RsaPublicKey) -> Vec<u8> {
    let cipher = BigUint::from_bytes_be(block);
    cipher.modpow(public_key.e(), public_key.n()).to_bytes_be()
}

fn is_encrypted_empty_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == 'A' || ch == '=')
}

fn is_mostly_printable(value: &str) -> bool {
    value
        .chars()
        .all(|ch| !ch.is_control() || matches!(ch, '\r' | '\n' | '\t'))
}

fn build_remote_folder(number: &str, name: &str) -> String {
    let number = number.trim();
    if number.is_empty() {
        return String::new();
    }

    let slug = slugify(name.trim());
    if slug.is_empty() {
        number.to_string()
    } else {
        format!("{number}-{slug}")
    }
}

fn slugify(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(value.len());
    let mut previous_dash = false;
    for ch in value.nfd() {
        if is_combining_mark(ch) {
            continue;
        }
        if ch.is_alphanumeric() {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }

    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        build_remote_folder, detect_customer_hint, is_encrypted_empty_placeholder, slugify,
        XD_LICENSE_PATH, XD_PEM_PATH,
    };
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn slugify_matches_helper_shape() {
        assert_eq!(slugify("Palmeira Minimercado"), "Palmeira-Minimercado");
        assert_eq!(slugify("Joao & Filhos, Lda."), "Joao-Filhos-Lda");
        assert_eq!(slugify("  ---  "), "");
    }

    #[test]
    fn build_remote_folder_uses_number_and_slug() {
        assert_eq!(
            build_remote_folder("XDPT.59655", "Palmeira Minimercado"),
            "XDPT.59655-Palmeira-Minimercado"
        );
        assert_eq!(build_remote_folder("XDPT.59655", ""), "XDPT.59655");
    }

    #[test]
    fn encrypted_empty_placeholder_is_detected() {
        assert!(is_encrypted_empty_placeholder("AAAA===="));
        assert!(!is_encrypted_empty_placeholder(""));
        assert!(!is_encrypted_empty_placeholder("ABCD="));
    }

    #[test]
    fn native_detection_matches_license_inspector_when_available() {
        let helper = Path::new(".\\license-inspector.exe");
        if !Path::new(XD_LICENSE_PATH).is_file()
            || !Path::new(XD_PEM_PATH).is_file()
            || !helper.is_file()
        {
            return;
        }

        let native = detect_customer_hint()
            .expect("native XD detection should succeed when XD licence files exist");
        let output = Command::new(helper)
            .arg("--remote-folder")
            .output()
            .expect("license-inspector.exe --remote-folder should run");
        assert!(
            output.status.success(),
            "license-inspector.exe --remote-folder failed"
        );
        let helper_folder = String::from_utf8(output.stdout)
            .expect("helper output should be UTF-8")
            .trim()
            .to_string();

        assert_eq!(native.folder, helper_folder);
    }
}
