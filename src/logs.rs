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
