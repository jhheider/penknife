use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::GlobSet;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Path relative to root, e.g. "Red Hand of Doom/rp-posts/Many Roads.md"
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub modified: SystemTime,
}

/// Text file extensions surfaced in the tree. Limited to formats that
/// round-trip cleanly through a gist (text, diffable). Binary formats
/// (pdf/png/etc.) are intentionally excluded — they don't fit the sync model.
pub fn is_supported_ext(ext: &str) -> bool {
    matches!(ext, "md" | "json")
}

/// Walk `root` recursively, collecting all supported files, sorted by mtime
/// (newest first). Skips files whose rel_path matches `ignore`.
pub fn scan_directory(root: &Path, ignore: &GlobSet) -> Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    walk_dir(root, root, ignore, &mut files)?;
    files.sort_by_key(|b| std::cmp::Reverse(b.modified));
    Ok(files)
}

fn walk_dir(root: &Path, dir: &Path, ignore: &GlobSet, out: &mut Vec<ScannedFile>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // entry vanished between read_dir and now
        };
        let path = entry.path();
        // Skip hidden entries
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            // Test the directory's rel_path against ignore patterns first —
            // skipping a dir avoids descending into a potentially large tree.
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if ignore.is_match(&rel) {
                continue;
            }
            let before = out.len();
            walk_dir(root, &path, ignore, out)?;
            // Prune directories that contained no supported files
            if out.len() == before {
                continue;
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(is_supported_ext)
        {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if ignore.is_match(&rel) {
                continue;
            }
            // If metadata fails, skip the file rather than fabricating a UNIX_EPOCH
            // mtime (which would surface as a stale-looking entry at the bottom).
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(ScannedFile {
                rel_path: rel,
                abs_path: path,
                modified,
            });
        }
    }
    Ok(())
}

/// Build a `GlobSet` from a list of pattern strings. Invalid patterns are
/// dropped with an `eprintln!` warning (the user sees them at startup before
/// the TUI takes over the terminal).
pub fn build_globset(patterns: &[String]) -> GlobSet {
    let mut builder = globset::GlobSetBuilder::new();
    for p in patterns {
        match globset::Glob::new(p) {
            Ok(g) => {
                builder.add(g);
            }
            Err(e) => {
                eprintln!("Ignoring invalid glob pattern {p:?}: {e}");
            }
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}
