//! Multi-file find-and-replace with per-match human review.
//!
//! Workflow: the caller asks `scan()` for all matches of `query` inside a
//! scope directory (recursive over .md files). The user reviews matches in
//! the UI, leaving some checked and unchecking false positives. Then
//! `apply()` is called with the *checked subset* and the replacement string.
//!
//! Apply is robust to file mutation between scan and apply: we re-read each
//! file and verify the query string is still at the recorded (line, col)
//! before touching it. If a match drifted, that single substitution is
//! skipped and counted in the result.

use std::path::{Path, PathBuf};

use crate::scanner;

/// One occurrence of `query` in a file.
#[derive(Debug, Clone)]
pub struct ReplaceMatch {
    /// Absolute path on disk (so we can read/write).
    pub abs_path: PathBuf,
    /// Path relative to the active root (for display).
    pub rel_path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// 0-indexed *byte* offset within the line where the match starts.
    /// Byte (not char) because Rust's `find` returns byte indices and that's
    /// what we need to slice when applying.
    pub col_byte: usize,
    /// The full text of the line, for context display in the review UI.
    pub line_text: String,
}

/// Result of a single apply pass.
#[derive(Debug, Default)]
pub struct ApplyResult {
    /// How many substitutions were actually written.
    pub applied: usize,
    /// How many checked matches we skipped because the file had drifted
    /// (e.g. the user edited the file in $EDITOR between scan and apply).
    pub drifted: usize,
    /// Files we couldn't read or write. The string is the OS error.
    pub errors: Vec<(PathBuf, String)>,
    /// Distinct files that were modified — useful for refresh + status text.
    pub files_changed: Vec<PathBuf>,
}

/// Scan `scope` recursively for occurrences of `query` in `.md` files.
/// `root` is the user-configured root (for computing rel_path display).
/// Empty `query` returns no matches.
pub fn scan(scope: &Path, root: &Path, query: &str) -> Vec<ReplaceMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    // Find/replace honors no ignore patterns of its own — if the user has
    // narrowed the picker (via [ignore] in config) those exclusions belong
    // to the *tree*, not to substring search across the directory.
    let files = match scanner::scan_directory(scope, &globset::GlobSet::empty()) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for f in files {
        let Ok(content) = std::fs::read_to_string(&f.abs_path) else {
            continue;
        };
        // Compute rel_path relative to *root*, not scope, so display stays
        // anchored to the configured root regardless of how deep the scope is.
        let rel_path = f
            .abs_path
            .strip_prefix(root)
            .unwrap_or(&f.abs_path)
            .to_string_lossy()
            .to_string();
        for (line_idx, line) in content.lines().enumerate() {
            let mut cursor = 0;
            while let Some(found) = line[cursor..].find(query) {
                let col = cursor + found;
                out.push(ReplaceMatch {
                    abs_path: f.abs_path.clone(),
                    rel_path: rel_path.clone(),
                    line: line_idx + 1,
                    col_byte: col,
                    line_text: line.to_string(),
                });
                cursor = col + query.len();
            }
        }
    }
    out
}

/// Apply the substitutions described by `matches` (already filtered to the
/// user's checked subset). Groups by file, sorts each file's hits by descending
/// (line, col) so earlier edits don't shift later offsets, then writes back.
pub fn apply(matches: &[ReplaceMatch], query: &str, target: &str) -> ApplyResult {
    let mut result = ApplyResult::default();
    if matches.is_empty() || query.is_empty() {
        return result;
    }

    // Group matches by file path.
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<&ReplaceMatch>> =
        std::collections::BTreeMap::new();
    for m in matches {
        by_file.entry(m.abs_path.clone()).or_default().push(m);
    }

    for (path, mut hits) in by_file {
        // Sort descending so byte offsets earlier in the file remain valid
        // after we mutate later ones.
        hits.sort_by_key(|h| std::cmp::Reverse((h.line, h.col_byte)));

        let original = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                result.errors.push((path, e.to_string()));
                continue;
            }
        };

        // Walk lines into owned Strings so we can mutate per-line.
        let mut lines: Vec<String> = original.split('\n').map(|s| s.to_string()).collect();
        let mut changed = false;
        for m in hits {
            let idx = m.line.saturating_sub(1);
            let Some(line) = lines.get_mut(idx) else {
                result.drifted += 1;
                continue;
            };
            // Drift check: the byte slice at col_byte must still equal query.
            let end = m.col_byte + query.len();
            if end > line.len() || &line[m.col_byte..end] != query {
                result.drifted += 1;
                continue;
            }
            line.replace_range(m.col_byte..end, target);
            result.applied += 1;
            changed = true;
        }

        if changed {
            let new_content = lines.join("\n");
            match std::fs::write(&path, new_content) {
                Ok(()) => result.files_changed.push(path),
                Err(e) => result.errors.push((path, e.to_string())),
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "wm-replace-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn scan_finds_multiple_matches_in_a_line() {
        let dir = tmp();
        fs::write(dir.join("a.md"), "Urslog met another Urslog.\n").unwrap();
        let m = scan(&dir, &dir, "Urslog");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].line, 1);
        assert_eq!(m[0].col_byte, 0);
        assert_eq!(m[1].col_byte, 19);
    }

    #[test]
    fn scan_walks_subdirectories() {
        let dir = tmp();
        fs::create_dir_all(dir.join("rp-posts")).unwrap();
        fs::create_dir_all(dir.join("session-notes")).unwrap();
        fs::write(dir.join("rp-posts/p1.md"), "Urslog grunts.\n").unwrap();
        fs::write(dir.join("session-notes/s1.md"), "The Urslog flees.\n").unwrap();
        let m = scan(&dir, &dir, "Urslog");
        assert_eq!(m.len(), 2);
        let paths: Vec<&str> = m.iter().map(|h| h.rel_path.as_str()).collect();
        assert!(paths.iter().any(|p| p.contains("rp-posts")));
        assert!(paths.iter().any(|p| p.contains("session-notes")));
    }

    #[test]
    fn apply_replaces_only_checked_matches() {
        let dir = tmp();
        fs::write(
            dir.join("a.md"),
            "Urslog appears.\nUrslog roars.\nUrslog flees.\n",
        )
        .unwrap();
        let mut hits = scan(&dir, &dir, "Urslog");
        assert_eq!(hits.len(), 3);
        // Keep only the middle hit checked.
        hits.retain(|h| h.line == 2);
        let r = apply(&hits, "Urslog", "Viewslog");
        assert_eq!(r.applied, 1);
        assert_eq!(r.drifted, 0);
        let content = fs::read_to_string(dir.join("a.md")).unwrap();
        assert_eq!(content, "Urslog appears.\nViewslog roars.\nUrslog flees.\n");
    }

    #[test]
    fn apply_multiple_matches_in_one_line_preserves_offsets() {
        let dir = tmp();
        fs::write(dir.join("a.md"), "foo foo foo\n").unwrap();
        let hits = scan(&dir, &dir, "foo");
        assert_eq!(hits.len(), 3);
        let r = apply(&hits, "foo", "BARBAZ"); // longer replacement
        assert_eq!(r.applied, 3);
        let content = fs::read_to_string(dir.join("a.md")).unwrap();
        assert_eq!(content, "BARBAZ BARBAZ BARBAZ\n");
    }

    #[test]
    fn apply_detects_drift_when_file_mutated_between_scan_and_apply() {
        let dir = tmp();
        fs::write(dir.join("a.md"), "Urslog roars.\n").unwrap();
        let hits = scan(&dir, &dir, "Urslog");
        // Mutate the file so the byte slice no longer matches.
        fs::write(dir.join("a.md"), "Goblin roars.\n").unwrap();
        let r = apply(&hits, "Urslog", "Viewslog");
        assert_eq!(r.applied, 0);
        assert_eq!(r.drifted, 1);
        let content = fs::read_to_string(dir.join("a.md")).unwrap();
        assert_eq!(content, "Goblin roars.\n");
    }

    #[test]
    fn empty_query_returns_no_matches() {
        let dir = tmp();
        fs::write(dir.join("a.md"), "Anything.\n").unwrap();
        assert_eq!(scan(&dir, &dir, "").len(), 0);
    }
}
