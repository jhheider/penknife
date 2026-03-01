use std::collections::BTreeMap;

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

#[derive(Debug, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub files: BTreeMap<String, FileEntry>,
}

impl Store {
    fn store_path() -> std::path::PathBuf {
        Config::data_dir().join("store.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::store_path();
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(Self {
                version: 1,
                files: BTreeMap::new(),
            })
        }
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

    pub fn get(&self, rel_path: &str) -> Option<&FileEntry> {
        self.files.get(rel_path)
    }

    pub fn insert(&mut self, rel_path: String, entry: FileEntry) {
        self.files.insert(rel_path, entry);
    }

    pub fn remove(&mut self, rel_path: &str) -> Option<FileEntry> {
        self.files.remove(rel_path)
    }
}
