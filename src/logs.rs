use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

static LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static REGISTERED_SECRETS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

/// Register a runtime credential for exact-value redaction. Values are kept in
/// memory only and never written to disk.
pub fn register_secret(value: &str) {
    let value = value.trim();
    if value.len() < 4 {
        return;
    }
    let secrets = REGISTERED_SECRETS.get_or_init(|| RwLock::new(Vec::new()));
    if let Ok(mut values) = secrets.write() {
        if !values.iter().any(|known| known == value) {
            values.push(value.to_string());
        }
    }
}

pub fn append(message: &str) {
    let message = redact(message).replace(['\r', '\n'], " ");
    let _guard = LOG_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = fs::create_dir_all(logs_dir());
    let path = log_file_path();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}  {}", timestamp(), message);
    }
}

fn redact(message: &str) -> String {
    let mut output = message.to_string();
    if let Some(secrets) = REGISTERED_SECRETS.get() {
        if let Ok(values) = secrets.read() {
            for secret in values.iter() {
                output = output.replace(secret, "[REDACTED]");
            }
        }
    }
    for key in [
        "device_token",
        "poll_token",
        "s3_secret_key",
        "secret_access_key",
        "s3_access_key",
        "authorization",
        "x-amz-security-token",
        "password",
    ] {
        output = redact_named_value(&output, key);
    }
    output
}

fn redact_named_value(input: &str, key: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) =
        find_ascii_case_insensitive(&input.as_bytes()[cursor..], key.as_bytes())
    {
        let key_start = cursor + relative;
        let key_end = key_start + key.len();
        output.push_str(&input[cursor..key_end]);
        let bytes = input.as_bytes();
        let mut separator = key_end;
        while separator < bytes.len() && matches!(bytes[separator], b'"' | b'\'' | b' ' | b'\t') {
            separator += 1;
        }
        if separator >= bytes.len() || !matches!(bytes[separator], b':' | b'=') {
            cursor = key_end;
            continue;
        }
        separator += 1;
        while separator < bytes.len() && matches!(bytes[separator], b' ' | b'\t') {
            separator += 1;
        }
        output.push_str(&input[key_end..separator]);
        let quote = bytes
            .get(separator)
            .copied()
            .filter(|b| *b == b'"' || *b == b'\'');
        if quote.is_some() {
            output.push(bytes[separator] as char);
            separator += 1;
        }
        output.push_str("[REDACTED]");
        let mut value_end = separator;
        while value_end < bytes.len() {
            let byte = bytes[value_end];
            if quote.map_or(matches!(byte, b',' | b' ' | b'\t' | b'}'), |q| byte == q) {
                break;
            }
            value_end += 1;
        }
        cursor = value_end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

pub fn ensure_logs_dir() -> PathBuf {
    let dir = logs_dir();
    let _ = fs::create_dir_all(&dir);
    dir
}

fn logs_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    #[cfg(target_os = "macos")]
    if is_macos_app_executable(&exe) {
        // Writing below Contents/MacOS invalidates the app's sealed signature.
        return crate::paths::app_support_dir().join("logs");
    }

    let mut dir = exe;
    dir.pop();
    dir.push("logs");
    dir
}

#[cfg(target_os = "macos")]
fn is_macos_app_executable(exe: &Path) -> bool {
    let Some(macos_dir) = exe.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(app_dir) = contents_dir.parent() else {
        return false;
    };
    macos_dir.file_name().is_some_and(|name| name == "MacOS")
        && contents_dir
            .file_name()
            .is_some_and(|name| name == "Contents")
        && app_dir
            .extension()
            .is_some_and(|extension| extension == "app")
}

fn log_file_path() -> PathBuf {
    let mut path = logs_dir();
    path.push(format!("{}.log", day_stamp()));
    path
}

fn day_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{is_macos_app_executable, redact, redact_named_value, register_secret};
    use std::path::Path;

    #[test]
    fn detects_only_executables_inside_app_macos_directory() {
        assert!(is_macos_app_executable(Path::new(
            "/Applications/Backup Sync Tool.app/Contents/MacOS/backupsynctool"
        )));
        assert!(!is_macos_app_executable(Path::new(
            "/usr/local/bin/backupsynctool"
        )));
        assert!(!is_macos_app_executable(Path::new(
            "/tmp/Fake.app/backupsynctool"
        )));
    }

    #[test]
    fn redacts_structured_and_registered_credentials() {
        register_secret("literal-secret-123");
        let line = redact(r#"device_token=abc s3_secret_key: \"def\", note=literal-secret-123"#);
        assert!(!line.contains("abc"));
        assert!(!line.contains("def"));
        assert!(!line.contains("literal-secret-123"));
        assert!(line.contains("note="));
    }

    #[test]
    fn leaves_non_secret_fields_intact() {
        assert_eq!(
            redact_named_value("status=approved remote_folder=Customer", "device_token"),
            "status=approved remote_folder=Customer"
        );
    }
}
