use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Path relative to root, e.g. "Red Hand of Doom/rp-posts/Many Roads.md"
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub modified: SystemTime,
}

/// Walk `root` recursively, collecting all `.md` files, sorted by mtime (newest first).
pub fn scan_directory(root: &Path) -> Result<Vec<ScannedFile>> {
    let mut files = Vec::new();
    walk_dir(root, root, &mut files)?;
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(files)
}

fn walk_dir(root: &Path, dir: &Path, out: &mut Vec<ScannedFile>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        // Skip hidden entries
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            let before = out.len();
            walk_dir(root, &path, out)?;
            // Prune directories that contained no markdown files
            if out.len() == before {
                continue;
            }
        } else if path.extension().is_some_and(|e| e == "md") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let modified = entry
                .metadata()?
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(ScannedFile {
                rel_path: rel,
                abs_path: path,
                modified,
            });
        }
    }
    Ok(())
}
