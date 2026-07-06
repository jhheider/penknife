use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use globset::GlobSet;

use crate::error::Result;

/// A rel_path in canonical form: path components joined with `/` on every
/// platform. Store keys and the tree's nesting both split on `/`, so a Windows
/// `\` separator must be normalized. A literal backslash inside a Unix
/// filename is preserved, because there it is part of a component, not a
/// separator.
pub fn rel_to_string(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Path relative to root, e.g. "Red Hand of Doom/rp-posts/Many Roads.md"
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub modified: SystemTime,
}

/// Text file extensions surfaced in the tree. Limited to formats that
/// round-trip cleanly through a gist (text, diffable). Binary formats
/// (pdf/png/etc.) are intentionally excluded - they don't fit the sync model.
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
            // Test the directory's rel_path against ignore patterns first -
            // skipping a dir avoids descending into a potentially large tree.
            let rel = rel_to_string(path.strip_prefix(root).unwrap_or(&path));
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
            let rel = rel_to_string(path.strip_prefix(root).unwrap_or(&path));
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

#[cfg(test)]
mod rel_tests {
    use super::rel_to_string;
    use std::path::Path;

    #[test]
    fn rel_to_string_joins_components_with_forward_slash() {
        // Built from components so the input uses the native separator; the
        // output is `/`-joined on every platform.
        let p: std::path::PathBuf = ["a", "b", "c.md"].iter().collect();
        assert_eq!(rel_to_string(&p), "a/b/c.md");
        assert_eq!(rel_to_string(Path::new("flat.md")), "flat.md");
    }
}
