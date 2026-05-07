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

        // Migrate v1 → v2: old format had `files: BTreeMap<String, FileEntry>` (root-less).
        // Attribute each entry to the configured root that actually contains the file;
        // fall back to the first configured root.
        let old_files: BTreeMap<String, FileEntry> = value
            .get("files")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();

        let cfg = Config::load().unwrap_or_default();
        let mut roots: BTreeMap<PathBuf, BTreeMap<String, FileEntry>> = BTreeMap::new();
        for (rel, entry) in old_files {
            let owner = cfg
                .roots
                .iter()
                .find(|r| r.join(&rel).is_file())
                .or_else(|| cfg.roots.first())
                .cloned();
            if let Some(owner) = owner {
                let owner = canonicalize_root(&owner);
                roots.entry(owner).or_default().insert(rel, entry);
            }
            // If no roots are configured at all, drop the orphans.
        }
        Ok(Self {
            version: CURRENT_VERSION,
            roots,
        })
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
