use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub fn append(message: &str) {
    let _ = fs::create_dir_all(logs_dir());
    let path = log_file_path();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}  {}", timestamp(), message);
    }
}

pub fn ensure_logs_dir() -> PathBuf {
    let dir = logs_dir();
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Newest-first raw lines from today's log (disk; unfiltered).
pub fn recent_lines(limit: usize) -> Vec<String> {
    let path = log_file_path();
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(limit)
        .map(|l| l.to_string())
        .collect()
}

/// UI feed for status window — Windows-style: successes/failures, no progress spam.
/// Newest-first. Dedupes per file so Uploading collapses under later Uploaded.
pub fn recent_activity_for_ui(limit: usize) -> Vec<String> {
    let pool = recent_lines(800);
    let mut out = Vec::new();
    let mut seen = Vec::new(); // small; order stable, no HashSet dep churn

    for line in pool {
        let msg = strip_log_prefix(&line);
        let Some(display) = format_activity_line(msg) else {
            continue;
        };
        if let Some(key) = display.dedupe_key {
            if seen.iter().any(|k| k == &key) {
                continue;
            }
            seen.push(key);
        }
        out.push(display.label);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Newest-first basenames from successful uploads. Empty → popover "No recent uploads".
pub fn recent_sync_lines(limit: usize) -> Vec<String> {
    let pool = recent_lines(800);
    let mut names = Vec::new();
    for line in pool {
        let msg = strip_log_prefix(&line);
        let path = match uploaded_path(msg) {
            Some(p) => p,
            None => continue,
        };
        let name = basename(path);
        if name.is_empty() || names.iter().any(|n| n == &name) {
            continue;
        }
        names.push(name);
        if names.len() >= limit {
            break;
        }
    }
    // Fallback: still-in-progress uploads if no Completed lines yet.
    if names.is_empty() {
        for line in recent_lines(200) {
            let msg = strip_log_prefix(&line);
            let Some(path) = msg.strip_prefix("Uploading: ") else {
                continue;
            };
            let name = basename(path);
            if name.is_empty() || names.iter().any(|n| n == &name) {
                continue;
            }
            names.push(name);
            if names.len() >= limit {
                break;
            }
        }
    }
    names
}

struct ActivityDisplay {
    label: String,
    /// When set, later older lines for same file are dropped.
    dedupe_key: Option<String>,
}

fn format_activity_line(msg: &str) -> Option<ActivityDisplay> {
    // Noise — stay on disk, never in UI.
    if msg.starts_with("Upload progress:") {
        return None;
    }

    if let Some(path) = uploaded_path(msg) {
        let name = basename(path);
        return Some(ActivityDisplay {
            label: format!("Uploaded {name}"),
            dedupe_key: Some(format!("up:{name}")),
        });
    }
    if let Some(rest) = msg.strip_prefix("Upload failed ") {
        let (relative, err) = rest.split_once(": ").unwrap_or((rest, ""));
        let name = basename(relative);
        let detail = err.trim();
        let label = if detail.is_empty() {
            format!("Failed {name}")
        } else {
            format!("Failed {name} — {detail}")
        };
        return Some(ActivityDisplay {
            label,
            dedupe_key: Some(format!("up:{name}")),
        });
    }
    if let Some(path) = msg.strip_prefix("Uploading: ") {
        let name = basename(path);
        return Some(ActivityDisplay {
            label: format!("Uploading {name}"),
            dedupe_key: Some(format!("up:{name}")),
        });
    }
    if let Some(path) = msg.strip_prefix("Downloaded: ") {
        let name = basename(path);
        return Some(ActivityDisplay {
            label: format!("Downloaded {name}"),
            dedupe_key: Some(format!("dl:{name}")),
        });
    }
    if let Some(path) = msg.strip_prefix("Downloading: ") {
        let name = basename(path);
        return Some(ActivityDisplay {
            label: format!("Downloading {name}"),
            dedupe_key: Some(format!("dl:{name}")),
        });
    }

    if is_useful_info(msg) {
        return Some(ActivityDisplay {
            label: msg.to_string(),
            dedupe_key: None,
        });
    }
    None
}

fn uploaded_path(msg: &str) -> Option<&str> {
    msg.strip_prefix("Uploaded: ")
        .or_else(|| msg.strip_prefix("Uploaded:"))
        .map(str::trim)
        .filter(|p| !p.is_empty())
}

fn is_useful_info(msg: &str) -> bool {
    msg.starts_with("Checking remote")
        || msg.starts_with("Counting local")
        || msg.starts_with("Comparing local")
        || msg.ends_with(" file(s) to upload")
        || msg.starts_with("Startup scan")
        || msg.starts_with("Sync engine")
        || msg.starts_with("Paired")
        || msg.starts_with("Pair ")
        || msg.starts_with("menubar:")
        || msg.starts_with("Update ")
        || msg.starts_with("updater:")
        || msg.starts_with("Restored")
        || msg.starts_with("Restore ")
        || msg.starts_with("Re-pair")
        || msg.starts_with("Server approved")
        || msg.starts_with("Watch")
        || msg.starts_with("Start at login")
        || msg.starts_with("! ")
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_string()
}

/// Drop leading `HH:MM:SS  ` timestamp(s) if present.
fn strip_log_prefix(line: &str) -> &str {
    let mut s = line;
    loop {
        let bytes = s.as_bytes();
        if bytes.len() >= 10
            && bytes[2] == b':'
            && bytes[5] == b':'
            && bytes[8] == b' '
            && bytes[9] == b' '
        {
            s = &s[10..];
            continue;
        }
        break;
    }
    s.trim()
}

fn logs_dir() -> PathBuf {
    let mut dir = std::env::current_exe().unwrap_or_default();
    dir.pop();
    dir.push("logs");
    dir
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
