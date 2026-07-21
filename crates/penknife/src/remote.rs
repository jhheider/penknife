//! Bulk remote-change detection.
//!
//! Push/pull/hydrate all record `remote_sha256` at sync time; nothing else
//! ever updates it, so edits made on gist.github.com (or another machine)
//! are invisible until checked. `check_remote` closes that gap: one
//! paginated listing, then a content fetch for only the gists whose
//! `updated_at` moved since we last looked.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use penknife_gist::GistClient;

use crate::store::FileEntry;
use crate::sync::sha256_hex;
use color_eyre::eyre::Result;

#[derive(Debug)]
pub struct RemoteCheckOutcome {
    /// Store entries whose remote-side fields need updating, keyed by
    /// rel_path. Includes both real divergences and bare `updated_at`
    /// refreshes (description edits, first-ever checks), persisting the
    /// latter is what keeps future checks cheap.
    pub updated: Vec<(String, FileEntry)>,
    /// rel_paths whose remote content now differs from the last-synced state.
    pub divergent: usize,
    /// rel_paths whose gist no longer exists remotely.
    pub missing: Vec<String>,
    /// Total mapped entries examined.
    pub checked: usize,
}

/// Compare every mapped file against the live gist listing and report which
/// entries' remote-side state changed. Pure with respect to the store: the
/// caller applies `updated` on the UI thread so concurrent push/pull results
/// aren't clobbered.
pub async fn check_remote(
    client: &GistClient,
    entries: &BTreeMap<String, FileEntry>,
    mut progress_cb: impl FnMut(usize, usize),
) -> Result<RemoteCheckOutcome> {
    let gists = client.list_all().await?;
    let by_id: HashMap<&str, &penknife_gist::Gist> =
        gists.iter().map(|g| (g.id.as_str(), g)).collect();

    let total = entries.len();
    let mut updated = Vec::new();
    let mut divergent = 0usize;
    let mut missing = Vec::new();

    for (i, (rel, entry)) in entries.iter().enumerate() {
        progress_cb(i + 1, total);
        let Some(listed) = by_id.get(entry.remote_id.as_str()) else {
            missing.push(rel.clone());
            continue;
        };
        if entry.remote_updated_at == Some(listed.updated_at) {
            continue; // unchanged since we last looked
        }

        // updated_at moved (or was never recorded); fetch the real content.
        // The listing omits content, so this needs a per-gist GET.
        let full = client.get(&entry.remote_id).await?;
        let filename = rel.rsplit('/').next().unwrap_or(rel);
        let remote_content = client
            .file_content(&full, filename)
            .await?
            .unwrap_or_default();
        let remote_sha256 = sha256_hex(&remote_content);

        if remote_sha256 != entry.remote_sha256 {
            divergent += 1;
        }
        let mut refreshed = entry.clone();
        refreshed.remote_sha256 = remote_sha256;
        refreshed.remote_updated_at = Some(full.updated_at);
        updated.push((rel.clone(), refreshed));
    }

    Ok(RemoteCheckOutcome {
        updated,
        divergent,
        missing,
        checked: total,
    })
}

/// Decide whether a freshly checked entry may overwrite the live store
/// entry. Rejects when the mapping changed identity (re-push under a new
/// gist, delete+recreate) or when a push/pull completed after the check
/// started; in both cases the live entry is the newer truth.
pub fn should_apply_update(
    current: Option<&FileEntry>,
    refreshed: &FileEntry,
    check_started: DateTime<Utc>,
) -> bool {
    match current {
        Some(cur) => cur.remote_id == refreshed.remote_id && cur.last_synced <= check_started,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(remote_id: &str, synced_at: i64) -> FileEntry {
        FileEntry {
            backend: crate::store::GIST_BACKEND.into(),
            remote_id: remote_id.into(),
            url: "u".into(),
            local_sha256: "l".into(),
            remote_sha256: "r".into(),
            last_synced: Utc.timestamp_opt(synced_at, 0).unwrap(),
            remote_updated_at: None,
        }
    }

    #[test]
    fn update_applies_to_unchanged_entry() {
        let started = Utc.timestamp_opt(100, 0).unwrap();
        let cur = entry("g1", 50);
        let refreshed = entry("g1", 50);
        assert!(should_apply_update(Some(&cur), &refreshed, started));
    }

    #[test]
    fn update_skipped_when_remote_id_changed() {
        let started = Utc.timestamp_opt(100, 0).unwrap();
        let cur = entry("g2", 50);
        let refreshed = entry("g1", 50);
        assert!(!should_apply_update(Some(&cur), &refreshed, started));
    }

    #[test]
    fn update_skipped_when_entry_synced_after_check_started() {
        let started = Utc.timestamp_opt(100, 0).unwrap();
        let cur = entry("g1", 200);
        let refreshed = entry("g1", 200);
        assert!(!should_apply_update(Some(&cur), &refreshed, started));
    }

    #[test]
    fn update_skipped_when_entry_removed() {
        let started = Utc.timestamp_opt(100, 0).unwrap();
        let refreshed = entry("g1", 50);
        assert!(!should_apply_update(None, &refreshed, started));
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const UPDATED_AT: &str = "2024-06-01T00:00:00Z";

    fn listed_gist(id: &str, filename: &str) -> serde_json::Value {
        // List-shaped: no content field, like the real /gists endpoint.
        serde_json::json!({
            "id": id,
            "html_url": format!("https://gist.github.com/u/{id}"),
            "description": null,
            "public": false,
            "files": {
                filename: { "filename": filename, "size": 10, "raw_url": null }
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": UPDATED_AT,
        })
    }

    fn full_gist(id: &str, filename: &str, content: &str) -> serde_json::Value {
        let mut v = listed_gist(id, filename);
        v["files"][filename]["content"] = content.into();
        v["files"][filename]["truncated"] = false.into();
        v
    }

    #[tokio::test]
    async fn check_remote_records_divergent_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![listed_gist("g1", "a.md")]))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full_gist(
                "g1",
                "a.md",
                "edited on web",
            )))
            .mount(&server)
            .await;
        let client = GistClient::with_base_url("t".into(), server.uri());
        let mut entries = BTreeMap::new();
        entries.insert("dir/a.md".to_string(), {
            let mut e = entry("g1", 0);
            e.remote_sha256 = sha256_hex("old content");
            e // remote_updated_at: None → forces a fetch
        });

        let outcome = check_remote(&client, &entries, |_, _| {}).await.unwrap();
        assert_eq!(outcome.checked, 1);
        assert_eq!(outcome.divergent, 1);
        assert!(outcome.missing.is_empty());
        let (rel, refreshed) = &outcome.updated[0];
        assert_eq!(rel, "dir/a.md");
        assert_eq!(refreshed.remote_sha256, sha256_hex("edited on web"));
        assert!(refreshed.remote_updated_at.is_some());
    }

    #[tokio::test]
    async fn check_remote_skips_gists_with_unchanged_updated_at() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![listed_gist("g1", "a.md")]))
            .mount(&server)
            .await;
        // A per-gist GET would 404 and fail the run; proves we skipped it.
        let client = GistClient::with_base_url("t".into(), server.uri());
        let mut entries = BTreeMap::new();
        entries.insert("a.md".to_string(), {
            let mut e = entry("g1", 0);
            e.remote_updated_at = Some(UPDATED_AT.parse().unwrap());
            e
        });

        let outcome = check_remote(&client, &entries, |_, _| {}).await.unwrap();
        assert_eq!(outcome.divergent, 0);
        assert!(outcome.updated.is_empty());
    }

    #[tokio::test]
    async fn check_remote_reports_deleted_gists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Vec::<serde_json::Value>::new()))
            .mount(&server)
            .await;
        let client = GistClient::with_base_url("t".into(), server.uri());
        let mut entries = BTreeMap::new();
        entries.insert("a.md".to_string(), entry("gone", 0));

        let outcome = check_remote(&client, &entries, |_, _| {}).await.unwrap();
        assert_eq!(outcome.missing, vec!["a.md".to_string()]);
        assert!(outcome.updated.is_empty());
    }
}
