//! `.bstignore` in the watch folder: paths this folder never syncs.
//!
//! One pattern per line; `#` starts a comment. Paths use `/`.
//! - `rollback/` — a folder (and everything in it), from the watch folder root.
//! - `*.tmp` — a pattern without `/` matches the file name anywhere.
//! - `models/*-massing.glb` — a pattern with `/` matches the whole path.
//!
//! Ignored files are never uploaded, downloaded or deleted, on either side.

use std::fs;
use std::path::Path;

pub const IGNORE_FILE: &str = ".bstignore";

#[derive(Debug, Default, Clone)]
pub struct Ignore {
    patterns: Vec<String>,
}

impl Ignore {
    pub fn load(root: &Path) -> Self {
        fs::read_to_string(root.join(IGNORE_FILE))
            .map(|raw| Self::parse(&raw))
            .unwrap_or_default()
    }

    pub fn parse(raw: &str) -> Self {
        let patterns = raw
            .lines()
            .map(|l| l.trim().trim_start_matches('/').replace('\\', "/"))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        Self { patterns }
    }

    pub fn matches(&self, rel: &str) -> bool {
        self.patterns.iter().any(|p| {
            if let Some(dir) = p.strip_suffix('/') {
                glob(dir, rel) || prefixes(rel).any(|pre| glob(dir, pre))
            } else if p.contains('/') {
                glob(p, rel)
            } else {
                rel.split('/').any(|part| glob(p, part))
            }
        })
    }
}

/// Every folder prefix of `rel`: "a/b/c.txt" -> "a", "a/b".
fn prefixes(rel: &str) -> impl Iterator<Item = &str> {
    rel.match_indices('/').map(move |(i, _)| &rel[..i])
}

/// `*` matches any run of characters except `/`; `?` one character.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' && t[ti] != '/' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star.filter(|_| t[mark] != '/') {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_and_path_patterns() {
        let ig = Ignore::parse("# snapshots\nrollback/\n*.tmp\nmodels/*-massing.glb\n");
        assert!(ig.matches("rollback/a.glb"));
        assert!(ig.matches("rollback/deep/a.glb"));
        assert!(!ig.matches("college-models/rollback.md"));
        assert!(!ig.matches("x/rollback/a.glb"));
        assert!(ig.matches("a/b/c.tmp"));
        assert!(ig.matches("models/JOH1-massing.glb"));
        assert!(!ig.matches("models/JOH1.glb"));
        assert!(!ig.matches("models/sub/JOH1-massing.glb"));
        assert!(!ig.matches(".bstignore"));
    }

    #[test]
    fn empty_ignores_nothing() {
        assert!(!Ignore::parse("").matches("a/b"));
    }
}
