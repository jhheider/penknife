//! UI-free application of async sync results to the store (and disk).
//!
//! These rules - when a finished pull may overwrite the working file, when
//! an observed remote divergence may overwrite the live store entry - are
//! the correctness core of the sync feature. They used to live inline in
//! `App::handle_async_event`, interleaved with status-bar text and tree
//! rebuilds, which made them untestable without a terminal. The App layer
//! now just renders the outcomes returned from here.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::store::{FileEntry, Store};
use crate::sync;

/// Outcome of applying a finished pull.
#[derive(Debug, PartialEq, Eq)]
pub enum PullApply {
    /// Remote content was written to disk and the store entry recorded.
    Applied,
    /// The file changed on disk while the pull was in flight (e.g. an
    /// `$EDITOR` session between confirm and completion); nothing was
    /// written and the store is untouched.
    DriftRefused,
}

/// Apply a completed pull: write `content` to `root/rel_path` and record
/// `entry`, unless the on-disk content no longer hashes to
/// `expected_local_sha256` - the snapshot taken when the pull started.
pub fn apply_pull(
    store: &mut Store,
    root: &Path,
    rel_path: &str,
    expected_local_sha256: &str,
    content: &str,
    entry: FileEntry,
) -> std::io::Result<PullApply> {
    let path = root.join(rel_path);
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    if sync::sha256_hex(&on_disk) != expected_local_sha256 {
        return Ok(PullApply::DriftRefused);
    }
    std::fs::write(&path, content)?;
    store.insert(root, rel_path.to_string(), entry);
    Ok(PullApply::Applied)
}

/// Record a remote divergence observed by a blocked push: update the
/// entry's remote-side fields so the tree shows RemoteNewer/Conflict even
/// if the user declines the force-push. Returns false (store untouched)
/// when the mapping no longer exists.
pub fn record_divergence(
    store: &mut Store,
    root: &Path,
    rel_path: &str,
    remote_sha256: String,
    remote_updated_at: DateTime<Utc>,
) -> bool {
    let Some(entry) = store.get(root, rel_path).cloned() else {
        return false;
    };
    let mut updated = entry;
    updated.remote_sha256 = remote_sha256;
    updated.remote_updated_at = Some(remote_updated_at);
    store.insert(root, rel_path.to_string(), updated);
    true
}

/// Record a remote observation from a single-file status check (the diff
/// fetch). Same bookkeeping as [`record_divergence`], but additionally
/// skipped when the entry synced after the check `started` - that sync
/// result is the newer truth. Returns true if the store changed.
pub fn record_observation(
    store: &mut Store,
    root: &Path,
    rel_path: &str,
    started: DateTime<Utc>,
    remote_sha256: String,
    remote_updated_at: DateTime<Utc>,
) -> bool {
    if store
        .get(root, rel_path)
        .filter(|e| e.last_synced <= started)
        .is_none()
    {
        return false;
    }
    record_divergence(store, root, rel_path, remote_sha256, remote_updated_at)
}

/// Merge a bulk remote check's refreshed entries into the store, skipping
/// any whose live entry changed identity or synced after the check
/// `started` (see [`crate::remote::should_apply_update`]). Returns the
/// rel_paths actually applied so the caller can refresh their cached
/// statuses.
pub fn apply_remote_updates(
    store: &mut Store,
    root: &Path,
    started: DateTime<Utc>,
    updated: Vec<(String, FileEntry)>,
) -> Vec<String> {
    let mut applied = Vec::new();
    for (rel, refreshed) in updated {
        if crate::remote::should_apply_update(store.get(root, &rel), &refreshed, started) {
            store.insert(root, rel.clone(), refreshed);
            applied.push(rel);
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(remote_id: &str, synced_at: i64) -> FileEntry {
        FileEntry {
            backend: crate::store::GIST_BACKEND.into(),
            remote_id: remote_id.into(),
            url: format!("https://gist.github.com/u/{remote_id}"),
            local_sha256: "l".into(),
            remote_sha256: "r".into(),
            last_synced: Utc.timestamp_opt(synced_at, 0).unwrap(),
            remote_updated_at: None,
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn pull_writes_file_and_records_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), "local").unwrap();
        let mut store = Store::default();

        let out = apply_pull(
            &mut store,
            root,
            "a.md",
            &sync::sha256_hex("local"),
            "remote content",
            entry("g1", 10),
        )
        .unwrap();

        assert_eq!(out, PullApply::Applied);
        assert_eq!(
            std::fs::read_to_string(root.join("a.md")).unwrap(),
            "remote content"
        );
        assert_eq!(store.get(root, "a.md").unwrap().remote_id, "g1");
    }

    #[test]
    fn pull_refuses_when_file_changed_mid_flight() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The pull started when the file said "local", but the user edited
        // it to "edited mid-pull" before the result arrived.
        std::fs::write(root.join("a.md"), "edited mid-pull").unwrap();
        let mut store = Store::default();

        let out = apply_pull(
            &mut store,
            root,
            "a.md",
            &sync::sha256_hex("local"),
            "remote content",
            entry("g1", 10),
        )
        .unwrap();

        assert_eq!(out, PullApply::DriftRefused);
        assert_eq!(
            std::fs::read_to_string(root.join("a.md")).unwrap(),
            "edited mid-pull"
        );
        assert!(store.get(root, "a.md").is_none());
    }

    #[test]
    fn divergence_updates_remote_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::default();
        store.insert(root, "a.md".into(), entry("g1", 10));

        assert!(record_divergence(
            &mut store,
            root,
            "a.md",
            "new-remote-sha".into(),
            at(50),
        ));
        let e = store.get(root, "a.md").unwrap();
        assert_eq!(e.remote_sha256, "new-remote-sha");
        assert_eq!(e.remote_updated_at, Some(at(50)));
        // Local-side fields are untouched.
        assert_eq!(e.local_sha256, "l");
    }

    #[test]
    fn divergence_noop_when_mapping_gone() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::default();
        assert!(!record_divergence(
            &mut store,
            dir.path(),
            "a.md",
            "x".into(),
            at(50),
        ));
    }

    #[test]
    fn observation_skipped_when_entry_synced_after_check_started() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::default();
        // Entry synced at t=200; the check started at t=100, so its
        // observation is stale.
        store.insert(root, "a.md".into(), entry("g1", 200));

        assert!(!record_observation(
            &mut store,
            root,
            "a.md",
            at(100),
            "stale-sha".into(),
            at(50),
        ));
        assert_eq!(store.get(root, "a.md").unwrap().remote_sha256, "r");
    }

    #[test]
    fn observation_applied_when_entry_older_than_check() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::default();
        store.insert(root, "a.md".into(), entry("g1", 10));

        assert!(record_observation(
            &mut store,
            root,
            "a.md",
            at(100),
            "observed-sha".into(),
            at(50),
        ));
        assert_eq!(
            store.get(root, "a.md").unwrap().remote_sha256,
            "observed-sha"
        );
    }

    #[test]
    fn remote_updates_apply_selectively() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = Store::default();
        // "old.md" hasn't synced since the check started → applies.
        store.insert(root, "old.md".into(), entry("g1", 10));
        // "fresh.md" synced after the check started → skipped.
        store.insert(root, "fresh.md".into(), entry("g2", 200));

        let mut refreshed_old = entry("g1", 10);
        refreshed_old.remote_sha256 = "new1".into();
        let mut refreshed_fresh = entry("g2", 200);
        refreshed_fresh.remote_sha256 = "new2".into();

        let applied = apply_remote_updates(
            &mut store,
            root,
            at(100),
            vec![
                ("old.md".into(), refreshed_old),
                ("fresh.md".into(), refreshed_fresh),
            ],
        );

        assert_eq!(applied, vec!["old.md".to_string()]);
        assert_eq!(store.get(root, "old.md").unwrap().remote_sha256, "new1");
        assert_eq!(store.get(root, "fresh.md").unwrap().remote_sha256, "r");
    }
}
