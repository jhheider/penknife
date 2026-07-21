//! Lightweight git integration: detect whether the active root is inside
//! a git repository, and (if so) query `git status --porcelain` to surface
//! staged/modified/untracked state in the tree. Read-only; never modifies
//! the repo from here. The few write-side commands (`git pull --rebase`,
//! `git push`) shell out via the existing suspend/resume pattern in main.rs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
}

impl GitStatus {
    #[allow(dead_code)] // useful enough that callers will reach for it
    pub fn is_clean(self) -> bool {
        !self.staged && !self.modified && !self.untracked
    }
}

/// Walk up from `start` looking for a `.git` entry. Returns the repo root
/// (the directory containing `.git`) if found.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Run `git status --porcelain=v1 -z` from `repo_root` and parse the output.
/// Returns a map keyed by path *relative to `repo_root`*; callers must
/// translate to their own rel_path space if `tree_root` differs.
///
/// On error (git not on PATH, repo too broken, etc.), returns an empty map.
/// Stays silent on errors because git-status is best-effort UI decoration.
pub fn status(repo_root: &Path) -> HashMap<String, GitStatus> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain=v1", "-z"])
        .output();
    let Ok(out) = output else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    parse_porcelain(&out.stdout)
}

/// Parse `git status --porcelain=v1 -z` output. Each record is
/// `XY <space> path \0`, with renamed entries adding `<old-path>\0` after.
fn parse_porcelain(bytes: &[u8]) -> HashMap<String, GitStatus> {
    let mut out: HashMap<String, GitStatus> = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        // Need at least "XY " (3 bytes) before the path.
        if i + 3 > bytes.len() {
            break;
        }
        let x = bytes[i] as char;
        let y = bytes[i + 1] as char;
        // bytes[i+2] is the space separator.
        i += 3;
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let path = match std::str::from_utf8(&bytes[start..i]) {
            Ok(s) => s.to_string(),
            Err(_) => {
                i += 1;
                continue;
            }
        };
        i += 1; // skip the NUL
        // Renamed entries (R..) carry an "old path\0" after the new path -
        // skip it so we don't try to parse it as the next record's XY.
        if x == 'R' || x == 'C' {
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
        }
        let status = GitStatus {
            staged: !matches!(x, ' ' | '?' | '!'),
            modified: !matches!(y, ' ' | '?' | '!'),
            untracked: x == '?' || y == '?',
        };
        out.insert(path, status);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain_modified() {
        // " M path\0"
        let bytes = b" M Red Hand of Doom/rp-posts/Posterity.md\0";
        let m = parse_porcelain(bytes);
        assert_eq!(m.len(), 1);
        let s = m.get("Red Hand of Doom/rp-posts/Posterity.md").unwrap();
        assert!(s.modified);
        assert!(!s.staged);
        assert!(!s.untracked);
    }

    #[test]
    fn parse_porcelain_staged_and_modified() {
        // "MM path\0": staged change *and* further unstaged tweaks
        let bytes = b"MM file.md\0";
        let m = parse_porcelain(bytes);
        let s = m.get("file.md").unwrap();
        assert!(s.staged);
        assert!(s.modified);
        assert!(!s.untracked);
    }

    #[test]
    fn parse_porcelain_untracked() {
        // "?? path\0"
        let bytes = b"?? new.md\0";
        let m = parse_porcelain(bytes);
        let s = m.get("new.md").unwrap();
        assert!(s.untracked);
        assert!(!s.staged);
        assert!(!s.modified);
    }

    #[test]
    fn parse_porcelain_multiple_records() {
        let bytes = b" M a.md\0?? b.md\0M  c.md\0";
        let m = parse_porcelain(bytes);
        assert_eq!(m.len(), 3);
        assert!(m["a.md"].modified);
        assert!(m["b.md"].untracked);
        assert!(m["c.md"].staged);
    }

    #[test]
    fn parse_porcelain_rename_skips_old_path() {
        // "R  newpath\0oldpath\0?? other.md\0"
        let bytes = b"R  newpath.md\0oldpath.md\0?? other.md\0";
        let m = parse_porcelain(bytes);
        // newpath staged-rename + other untracked. oldpath should NOT be a key.
        assert!(m.contains_key("newpath.md"));
        assert!(m.contains_key("other.md"));
        assert!(!m.contains_key("oldpath.md"));
    }
}
