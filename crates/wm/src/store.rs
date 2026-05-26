use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub gist_id: String,
    pub url: String,
    pub local_sha256: String,
    pub remote_sha256: String,
    pub last_synced: DateTime<Utc>,
}

const CURRENT_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Per-root file maps. Key is the canonical absolute path of the root directory.
    pub roots: BTreeMap<PathBuf, BTreeMap<String, FileEntry>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            roots: BTreeMap::new(),
        }
    }
}

impl Store {
    fn store_path() -> PathBuf {
        Config::data_dir().join("store.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::store_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&data)?;
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        if version >= CURRENT_VERSION {
            return Ok(serde_json::from_value(value)?);
        }

        // Migrate v1 → v2 in memory.
        let cfg = Config::load().unwrap_or_default();
        let migrated = migrate_v1_value(&value, &cfg.roots)?;
        // Persist the migrated format so subsequent loads are cheap and the
        // file no longer mentions v1.
        if let Err(e) = migrated.save() {
            eprintln!("warning: failed to persist migrated store: {e}");
        }
        Ok(migrated)
    }

    pub fn save(&self) -> Result<()> {
        let dir = Config::data_dir();
        std::fs::create_dir_all(&dir)?;
        let data = serde_json::to_string_pretty(self)?;

        // Atomic write: write to temp file then rename
        let path = Self::store_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn get(&self, root: &Path, rel_path: &str) -> Option<&FileEntry> {
        let key = canonicalize_root(root);
        self.roots.get(&key)?.get(rel_path)
    }

    pub fn insert(&mut self, root: &Path, rel_path: String, entry: FileEntry) {
        let key = canonicalize_root(root);
        self.roots.entry(key).or_default().insert(rel_path, entry);
    }

    /// Drop a single (root, rel_path) entry. No-op if it didn't exist.
    pub fn remove(&mut self, root: &Path, rel_path: &str) {
        let key = canonicalize_root(root);
        if let Some(map) = self.roots.get_mut(&key) {
            map.remove(rel_path);
        }
    }

    pub fn files_for_root(&self, root: &Path) -> Option<&BTreeMap<String, FileEntry>> {
        let key = canonicalize_root(root);
        self.roots.get(&key)
    }

    /// Merge entries from another store into this one, root-by-root.
    /// Existing entries in `self` are overwritten by entries from `other`.
    pub fn merge_from(&mut self, other: &Store) {
        for (root, files) in &other.roots {
            let entry = self.roots.entry(root.clone()).or_default();
            for (rel, file_entry) in files {
                entry.insert(rel.clone(), file_entry.clone());
            }
        }
    }
}

/// Canonicalize a root path for use as a stable map key.
/// Falls back to the original path if canonicalization fails (e.g. path doesn't exist).
fn canonicalize_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Migrate a v1-shaped JSON value into a v2 Store.
///
/// Old format had `files: BTreeMap<String, FileEntry>` (root-less). For each
/// entry, attribute it to the configured root whose `root.join(rel).is_file()`
/// matches; otherwise fall back to the first configured root. Drop entries
/// outright if no roots are configured (and warn on stderr).
fn migrate_v1_value(value: &serde_json::Value, roots: &[PathBuf]) -> Result<Store> {
    let old_files: BTreeMap<String, FileEntry> = value
        .get("files")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();

    let mut grouped: BTreeMap<PathBuf, BTreeMap<String, FileEntry>> = BTreeMap::new();
    let mut dropped = 0usize;
    for (rel, entry) in old_files {
        let owner = roots
            .iter()
            .find(|r| r.join(&rel).is_file())
            .or_else(|| roots.first())
            .cloned();
        if let Some(owner) = owner {
            grouped
                .entry(canonicalize_root(&owner))
                .or_default()
                .insert(rel, entry);
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        eprintln!("warning: dropped {dropped} entries from v1 store with no configured roots");
    }
    Ok(Store {
        version: CURRENT_VERSION,
        roots: grouped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_entry(id: &str) -> FileEntry {
        FileEntry {
            gist_id: id.into(),
            url: format!("https://gist.github.com/u/{id}"),
            local_sha256: "abc".into(),
            remote_sha256: "abc".into(),
            last_synced: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    #[test]
    fn migrate_drops_entries_when_no_roots_configured() {
        let v1 = serde_json::json!({
            "version": 1,
            "files": {
                "drafts/post.md": serde_json::to_value(sample_entry("g1")).unwrap(),
            }
        });
        let store = migrate_v1_value(&v1, &[]).unwrap();
        assert_eq!(store.version, CURRENT_VERSION);
        assert!(store.roots.is_empty());
    }

    #[test]
    fn migrate_attributes_to_first_root_as_fallback() {
        let v1 = serde_json::json!({
            "version": 1,
            "files": {
                "drafts/post.md": serde_json::to_value(sample_entry("g1")).unwrap(),
            }
        });
        let roots = vec![PathBuf::from("/nonexistent/root-a")];
        let store = migrate_v1_value(&v1, &roots).unwrap();
        assert_eq!(store.roots.len(), 1);
        let canonical = canonicalize_root(&roots[0]);
        let bucket = store.roots.get(&canonical).expect("first root present");
        assert!(bucket.contains_key("drafts/post.md"));
    }

    #[test]
    fn migrate_handles_missing_files_field() {
        let v1 = serde_json::json!({ "version": 1 });
        let store = migrate_v1_value(&v1, &[]).unwrap();
        assert!(store.roots.is_empty());
    }

    #[test]
    fn merge_from_overwrites_per_root() {
        let mut a = Store::default();
        let mut b = Store::default();
        let root = PathBuf::from("/r");
        a.insert(&root, "x.md".into(), sample_entry("old"));
        b.insert(&root, "x.md".into(), sample_entry("new"));
        a.merge_from(&b);
        assert_eq!(a.get(&root, "x.md").unwrap().gist_id, "new");
    }

    #[test]
    fn get_returns_none_for_unknown_root() {
        let store = Store::default();
        assert!(store.get(Path::new("/missing"), "anything.md").is_none());
    }
}
