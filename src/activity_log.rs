//! The last 24 hours of finished activity rows, kept in one file so the list
//! survives a restart. Both UIs read it. Lines are `<unix secs>\t<line>`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Rows older than this drop out of the list and the file.
pub const KEEP_SECS: u64 = 24 * 60 * 60;
/// At most this many rows are kept.
pub const MAX_ROWS: usize = 5000;

/// Rows written since the file was last trimmed.
static APPENDS: AtomicUsize = AtomicUsize::new(0);

fn path() -> PathBuf {
    crate::paths::app_support_dir().join("activity.log")
}

/// True for a line that ends a row (done, failed, or a notice). Rows that
/// still move are not kept.
pub fn is_finished(line: &str) -> bool {
    use crate::app::Activity;
    matches!(
        crate::app::parse_activity(line),
        Some(Activity::File { done: true, .. } | Activity::Failed { .. } | Activity::Info(_))
    )
}

/// Rows from the last 24 hours, oldest first.
pub fn read() -> Vec<(u64, String)> {
    let cutoff = crate::app::unix_now().saturating_sub(KEEP_SECS);
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let mut lines: Vec<(u64, String)> = text
        .lines()
        .filter_map(|l| {
            let (at, msg) = l.split_once('\t')?;
            Some((at.parse().ok()?, msg.to_string()))
        })
        .filter(|(at, _)| *at >= cutoff)
        .collect();
    let extra = lines.len().saturating_sub(MAX_ROWS);
    lines.drain(..extra);
    lines
}

/// Rewrite the file with the last 24 hours only. Returns those rows.
pub fn trim() -> Vec<(u64, String)> {
    let lines = read();
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = lines.iter().map(|(at, msg)| format!("{at}\t{msg}\n")).collect();
    let _ = std::fs::write(path, text);
    APPENDS.store(0, Ordering::Relaxed);
    lines
}

pub fn append(at: u64, line: &str) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{at}\t{}", line.replace(['\n', '\r'], " "));
    }
    if APPENDS.fetch_add(1, Ordering::Relaxed) >= 1000 {
        trim();
    }
}
