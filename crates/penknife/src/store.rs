use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use color_eyre::eyre::Result;

/// Backend name for GitHub Gists, the founding backend. Matches
/// `penknife_backend::Backend::name()` for the gist implementation.
pub const GIST_BACKEND: &str = "gist";

/// One published copy of a local file on some backend service. A file may
/// have several (one per backend); each tracks its own drift independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCopy {
    /// Which backend holds this copy, e.g. "gist".
    #[serde(default = "default_backend")]
    pub backend: String,
    /// The backend's identifier for this copy (the gist ID, the Doc ID, ...).
    /// `alias` keeps v1/v2 stores (which called it `gist_id`) parseable.
    #[serde(alias = "gist_id")]
    pub remote_id: String,
    pub url: String,
    pub local_sha256: String,
    pub remote_sha256: String,
    pub last_synced: DateTime<Utc>,
    /// The remote's revision timestamp as of the last time we observed its
    /// content. Lets the remote check skip fetching copies that haven't
    /// changed. `None` (e.g. entries from older store versions) forces a
    /// fetch on the next check.
    #[serde(default)]
    pub remote_updated_at: Option<DateTime<Utc>>,
}

fn default_backend() -> String {
    GIST_BACKEND.to_string()
}

/// Compatibility alias: most of the app still works with one (gist) copy
/// per file and calls it a FileEntry.
pub type FileEntry = RemoteCopy;

const CURRENT_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Per-root file maps. Key is the canonical absolute path of the root
    /// directory; each file maps to its list of published copies.
    pub roots: BTreeMap<PathBuf, BTreeMap<String, Vec<RemoteCopy>>>,
    /// Per-root timestamp of the last successful hydration walk, keyed by
    /// canonical root path. On the next hydrate for that root we pass it as
    /// the GitHub `since=` filter to fetch only gists updated after it,
    /// instead of re-listing every gist. The cursor is per-root (not global)
    /// because gists are account-wide: a gist that didn't match root A's
    /// files may still match root B's, so each root must do its own full
    /// first walk before going incremental. A missing entry forces a full
    /// walk, which then records the cursor.
    #[serde(default)]
    pub last_hydrated: BTreeMap<PathBuf, DateTime<Utc>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            roots: BTreeMap::new(),
            last_hydrated: BTreeMap::new(),
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

        // Older formats migrate in memory: v1 → v2 (root-keyed maps), then
        // v2 → v3 (single entry → list of copies).
        let cfg = Config::load().unwrap_or_default();
        let root_paths: Vec<PathBuf> = cfg.roots.iter().map(|r| r.path.clone()).collect();
        let v2 = if version >= 2 {
            migrate_v2_value(&value)?
        } else {
            migrate_v1_value(&value, &root_paths)?
        };
        let migrated = wrap_v2(v2);
        // Persist the migrated format so subsequent loads are cheap and the
        // file no longer mentions the old version.
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

    /// The gist copy for a file, if it has one. Gist-centric compatibility
    /// accessor; backend-aware callers should use [`Store::get_backend`].
    pub fn get(&self, root: &Path, rel_path: &str) -> Option<&RemoteCopy> {
        self.get_backend(root, rel_path, GIST_BACKEND)
    }

    /// The copy of `rel_path` held by `backend`, if any.
    pub fn get_backend(&self, root: &Path, rel_path: &str, backend: &str) -> Option<&RemoteCopy> {
        let key = canonicalize_root(root);
        self.roots
            .get(&key)?
            .get(rel_path)?
            .iter()
            .find(|c| c.backend == backend)
    }

    /// Upsert a copy: replaces the file's existing copy on the same backend,
    /// or appends if this backend had none.
    pub fn insert(&mut self, root: &Path, rel_path: String, copy: RemoteCopy) {
        let key = canonicalize_root(root);
        let copies = self
            .roots
            .entry(key)
            .or_default()
            .entry(rel_path)
            .or_default();
        if let Some(existing) = copies.iter_mut().find(|c| c.backend == copy.backend) {
            *existing = copy;
        } else {
            copies.push(copy);
        }
    }

    /// Drop the gist copy of a (root, rel_path). Other backends' copies are
    /// kept; the file's key disappears once its last copy is removed. No-op
    /// if it didn't exist.
    pub fn remove(&mut self, root: &Path, rel_path: &str) {
        self.remove_backend(root, rel_path, GIST_BACKEND);
    }

    /// Drop one backend's copy of a (root, rel_path).
    pub fn remove_backend(&mut self, root: &Path, rel_path: &str, backend: &str) {
        let key = canonicalize_root(root);
        if let Some(map) = self.roots.get_mut(&key)
            && let Some(copies) = map.get_mut(rel_path)
        {
            copies.retain(|c| c.backend != backend);
            if copies.is_empty() {
                map.remove(rel_path);
            }
        }
    }

    /// Move all of a file's copies from `old_rel` to `new_rel` (rename).
    pub fn move_entry(&mut self, root: &Path, old_rel: &str, new_rel: String) {
        let key = canonicalize_root(root);
        if let Some(map) = self.roots.get_mut(&key)
            && let Some(copies) = map.remove(old_rel)
        {
            map.insert(new_rel, copies);
        }
    }

    /// The full per-file copy lists for a root. Callers that only care about
    /// gists want [`Store::gist_entries_for_root`].
    pub fn files_for_root(&self, root: &Path) -> Option<&BTreeMap<String, Vec<RemoteCopy>>> {
        let key = canonicalize_root(root);
        self.roots.get(&key)
    }

    /// Owned map of rel_path → gist copy for a root, skipping files whose
    /// copies live only on other backends.
    pub fn gist_entries_for_root(&self, root: &Path) -> BTreeMap<String, RemoteCopy> {
        self.files_for_root(root)
            .map(|map| {
                map.iter()
                    .filter_map(|(rel, copies)| {
                        copies
                            .iter()
                            .find(|c| c.backend == GIST_BACKEND)
                            .map(|c| (rel.clone(), c.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The incremental-hydration cursor for `root`, if one has been recorded.
    pub fn hydrated_cursor(&self, root: &Path) -> Option<DateTime<Utc>> {
        let key = canonicalize_root(root);
        self.last_hydrated.get(&key).copied()
    }

    /// Record `ts` as the hydration cursor for `root` (set after a successful walk).
    pub fn set_hydrated_cursor(&mut self, root: &Path, ts: DateTime<Utc>) {
        let key = canonicalize_root(root);
        self.last_hydrated.insert(key, ts);
    }

    /// Merge entries from another store into this one, root-by-root.
    /// Copies from `other` overwrite same-backend copies in `self`; copies
    /// on backends `other` doesn't mention are kept.
    pub fn merge_from(&mut self, other: &Store) {
        for (root, files) in &other.roots {
            for (rel, copies) in files {
                for copy in copies {
                    self.insert(root, rel.clone(), copy.clone());
                }
            }
        }
    }
}

/// Canonicalize a root path for use as a stable map key.
/// Falls back to the original path if canonicalization fails (e.g. path doesn't exist).
fn canonicalize_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The v2 on-disk shape: one entry per file, implicitly a gist.
type V2Roots = BTreeMap<PathBuf, BTreeMap<String, RemoteCopy>>;

struct V2Store {
    roots: V2Roots,
    last_hydrated: BTreeMap<PathBuf, DateTime<Utc>>,
}

/// Parse a v2-shaped JSON value (single gist entry per file).
fn migrate_v2_value(value: &serde_json::Value) -> Result<V2Store> {
    let roots: V2Roots = value
        .get("roots")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let last_hydrated: BTreeMap<PathBuf, DateTime<Utc>> = value
        .get("last_hydrated")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    Ok(V2Store {
        roots,
        last_hydrated,
    })
}

/// Wrap a v2 store into v3: each single entry becomes a one-element copy
/// list, tagged as the gist backend.
fn wrap_v2(v2: V2Store) -> Store {
    let roots = v2
        .roots
        .into_iter()
        .map(|(root, files)| {
            let files = files
                .into_iter()
                .map(|(rel, mut entry)| {
                    entry.backend = GIST_BACKEND.to_string();
                    (rel, vec![entry])
                })
                .collect();
            (root, files)
        })
        .collect();
    Store {
        version: CURRENT_VERSION,
        roots,
        last_hydrated: v2.last_hydrated,
    }
}

/// Migrate a v1-shaped JSON value into the v2 shape.
///
/// Old format had `files: BTreeMap<String, FileEntry>` (root-less). For each
/// entry, attribute it to the configured root whose `root.join(rel).is_file()`
/// matches; otherwise fall back to the first configured root. Drop entries
/// outright if no roots are configured (and warn on stderr).
fn migrate_v1_value(value: &serde_json::Value, roots: &[PathBuf]) -> Result<V2Store> {
    let old_files: BTreeMap<String, RemoteCopy> = value
        .get("files")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();

    let mut grouped: V2Roots = BTreeMap::new();
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
    Ok(V2Store {
        roots: grouped,
        last_hydrated: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_entry(id: &str) -> RemoteCopy {
        RemoteCopy {
            backend: GIST_BACKEND.into(),
            remote_id: id.into(),
            url: format!("https://gist.github.com/u/{id}"),
            local_sha256: "abc".into(),
            remote_sha256: "abc".into(),
            last_synced: Utc.timestamp_opt(0, 0).unwrap(),
            remote_updated_at: None,
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
        let store = wrap_v2(migrate_v1_value(&v1, &[]).unwrap());
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
        let store = wrap_v2(migrate_v1_value(&v1, &roots).unwrap());
        assert_eq!(store.roots.len(), 1);
        let canonical = canonicalize_root(&roots[0]);
        let bucket = store.roots.get(&canonical).expect("first root present");
        assert!(bucket.contains_key("drafts/post.md"));
    }

    #[test]
    fn migrate_handles_missing_files_field() {
        let v1 = serde_json::json!({ "version": 1 });
        let store = wrap_v2(migrate_v1_value(&v1, &[]).unwrap());
        assert!(store.roots.is_empty());
    }

    #[test]
    fn migrate_v2_wraps_entries_as_gist_copies() {
        // A realistic v2 file: the id field is named gist_id and there is
        // no backend field anywhere.
        let v2 = serde_json::json!({
            "version": 2,
            "roots": {
                "/r": {
                    "post.md": {
                        "gist_id": "g1",
                        "url": "https://gist.github.com/u/g1",
                        "local_sha256": "abc",
                        "remote_sha256": "abc",
                        "last_synced": "2024-01-01T00:00:00Z"
                    }
                }
            },
            "last_hydrated": { "/r": "2024-06-01T00:00:00Z" }
        });
        let store = wrap_v2(migrate_v2_value(&v2).unwrap());
        assert_eq!(store.version, 3);
        let root = PathBuf::from("/r");
        let copy = store.get(&root, "post.md").expect("entry survives");
        assert_eq!(copy.backend, GIST_BACKEND);
        assert_eq!(copy.remote_id, "g1");
        assert!(store.hydrated_cursor(&root).is_some());
    }

    #[test]
    fn insert_upserts_by_backend() {
        let mut store = Store::default();
        let root = PathBuf::from("/r");
        store.insert(&root, "x.md".into(), sample_entry("old"));
        store.insert(&root, "x.md".into(), sample_entry("new"));
        // Same backend: replaced, not appended.
        assert_eq!(store.files_for_root(&root).unwrap()["x.md"].len(), 1);
        assert_eq!(store.get(&root, "x.md").unwrap().remote_id, "new");

        // A different backend coexists.
        let mut gdoc = sample_entry("d1");
        gdoc.backend = "gdoc".into();
        store.insert(&root, "x.md".into(), gdoc);
        assert_eq!(store.files_for_root(&root).unwrap()["x.md"].len(), 2);
        assert_eq!(store.get(&root, "x.md").unwrap().remote_id, "new");
        assert_eq!(
            store.get_backend(&root, "x.md", "gdoc").unwrap().remote_id,
            "d1"
        );
    }

    #[test]
    fn remove_only_touches_named_backend() {
        let mut store = Store::default();
        let root = PathBuf::from("/r");
        store.insert(&root, "x.md".into(), sample_entry("g1"));
        let mut gdoc = sample_entry("d1");
        gdoc.backend = "gdoc".into();
        store.insert(&root, "x.md".into(), gdoc);

        store.remove(&root, "x.md"); // gist compat remove
        assert!(store.get(&root, "x.md").is_none());
        assert!(store.get_backend(&root, "x.md", "gdoc").is_some());

        store.remove_backend(&root, "x.md", "gdoc");
        // Last copy gone: the key disappears entirely.
        assert!(
            !store
                .files_for_root(&root)
                .is_some_and(|m| m.contains_key("x.md"))
        );
    }

    #[test]
    fn move_entry_carries_all_copies() {
        let mut store = Store::default();
        let root = PathBuf::from("/r");
        store.insert(&root, "old.md".into(), sample_entry("g1"));
        let mut gdoc = sample_entry("d1");
        gdoc.backend = "gdoc".into();
        store.insert(&root, "old.md".into(), gdoc);

        store.move_entry(&root, "old.md", "new.md".into());
        assert!(store.get(&root, "old.md").is_none());
        assert_eq!(store.get(&root, "new.md").unwrap().remote_id, "g1");
        assert_eq!(
            store
                .get_backend(&root, "new.md", "gdoc")
                .unwrap()
                .remote_id,
            "d1"
        );
    }

    #[test]
    fn merge_from_overwrites_per_backend() {
        let mut a = Store::default();
        let mut b = Store::default();
        let root = PathBuf::from("/r");
        a.insert(&root, "x.md".into(), sample_entry("old"));
        // a also has a gdoc copy that b knows nothing about.
        let mut gdoc = sample_entry("d1");
        gdoc.backend = "gdoc".into();
        a.insert(&root, "x.md".into(), gdoc);
        b.insert(&root, "x.md".into(), sample_entry("new"));
        a.merge_from(&b);
        assert_eq!(a.get(&root, "x.md").unwrap().remote_id, "new");
        // The gdoc copy survives the merge.
        assert!(a.get_backend(&root, "x.md", "gdoc").is_some());
    }

    #[test]
    fn get_returns_none_for_unknown_root() {
        let store = Store::default();
        assert!(store.get(Path::new("/missing"), "anything.md").is_none());
    }

    #[test]
    fn gist_entries_skips_other_backends() {
        let mut store = Store::default();
        let root = PathBuf::from("/r");
        store.insert(&root, "a.md".into(), sample_entry("g1"));
        let mut gdoc = sample_entry("d1");
        gdoc.backend = "gdoc".into();
        store.insert(&root, "b.md".into(), gdoc);
        let gists = store.gist_entries_for_root(&root);
        assert_eq!(gists.len(), 1);
        assert!(gists.contains_key("a.md"));
    }

    #[test]
    fn hydration_cursor_is_per_root() {
        let mut store = Store::default();
        let a = PathBuf::from("/r-a");
        let b = PathBuf::from("/r-b");
        assert!(store.hydrated_cursor(&a).is_none());
        let t = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        store.set_hydrated_cursor(&a, t);
        assert_eq!(store.hydrated_cursor(&a), Some(t));
        // Setting one root's cursor must not bleed into another.
        assert!(store.hydrated_cursor(&b).is_none());
    }
}
