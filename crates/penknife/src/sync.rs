use sha2::{Digest, Sha256};

use chrono::Utc;
use gist_rs::GistClient;
use ratatui::style::Color;

use crate::error::Result;
use crate::store::FileEntry;

/// SHA-256 hash of content as hex string.
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Synced,
    LocalNewer,
    RemoteNewer,
    Conflict,
    NotGisted,
}

impl SyncStatus {
    pub fn icon(self) -> &'static str {
        let g = crate::glyphs::glyphs();
        match self {
            Self::Synced => g.status_synced,
            Self::LocalNewer => g.status_local_newer,
            Self::RemoteNewer => g.status_remote_newer,
            Self::Conflict => g.status_conflict,
            Self::NotGisted => g.status_not_gisted,
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Synced => Color::Green,
            Self::LocalNewer => Color::Yellow,
            Self::RemoteNewer => Color::Blue,
            Self::Conflict => Color::Red,
            Self::NotGisted => Color::DarkGray,
        }
    }
}

/// Determine sync status from local content and stored hashes.
pub fn local_status(local_content: &str, entry: Option<&FileEntry>) -> SyncStatus {
    let Some(entry) = entry else {
        return SyncStatus::NotGisted;
    };
    let local_hash = sha256_hex(local_content);
    let local_changed = local_hash != entry.local_sha256;
    let remote_changed =
        entry.remote_sha256 != entry.local_sha256 && entry.remote_sha256 != local_hash;

    match (local_changed, remote_changed) {
        (false, false) => SyncStatus::Synced,
        (true, false) => SyncStatus::LocalNewer,
        (false, true) => SyncStatus::RemoteNewer,
        (true, true) => SyncStatus::Conflict,
    }
}

/// Result of a remote-inclusive status check.
#[derive(Debug)]
pub struct FullStatus {
    pub status: SyncStatus,
    pub remote_content: String,
    pub remote_sha256: String,
    pub remote_updated_at: chrono::DateTime<Utc>,
}

/// Full status check including remote fetch.
pub async fn full_status(
    client: &GistClient,
    local_content: &str,
    entry: &FileEntry,
    filename: &str,
) -> Result<FullStatus> {
    let gist = client.get(&entry.remote_id).await?;
    let remote_content = client
        .file_content(&gist, filename)
        .await?
        .unwrap_or_default();
    let remote_hash = sha256_hex(&remote_content);
    let local_hash = sha256_hex(local_content);

    let local_changed = local_hash != entry.local_sha256;
    let remote_changed = remote_hash != entry.remote_sha256;

    let status = match (local_changed, remote_changed) {
        (false, false) => SyncStatus::Synced,
        (true, false) => SyncStatus::LocalNewer,
        (false, true) => SyncStatus::RemoteNewer,
        (true, true) => SyncStatus::Conflict,
    };

    Ok(FullStatus {
        status,
        remote_content,
        remote_sha256: remote_hash,
        remote_updated_at: gist.updated_at,
    })
}

/// Outcome of a push attempt.
pub enum PushOutcome {
    Pushed(FileEntry),
    /// The remote diverged from the last-synced state, so nothing was pushed.
    /// Carries the freshly observed remote state so the caller can surface
    /// the conflict and offer a force-push.
    RemoteChanged {
        remote_sha256: String,
        remote_updated_at: chrono::DateTime<Utc>,
    },
}

/// Push local content to gist (create or update). Unless `force` is set,
/// an update first fetches the remote and refuses to overwrite content that
/// changed since the last sync (lost-update guard).
///
/// On create, the gist's description is set to `filename` (the basename) - not
/// the rel_path - because gists are shared to game-specific channels where the
/// containing tree's directory structure leaks irrelevant context.
pub async fn push(
    client: &GistClient,
    existing: Option<&FileEntry>,
    filename: &str,
    content: &str,
    force: bool,
) -> Result<PushOutcome> {
    let hash = sha256_hex(content);
    let now = Utc::now();

    let gist = if let Some(entry) = existing {
        if !force {
            let current = client.get(&entry.remote_id).await?;
            let remote_content = client
                .file_content(&current, filename)
                .await?
                .unwrap_or_default();
            let remote_hash = sha256_hex(&remote_content);
            if remote_hash != entry.remote_sha256 && remote_hash != hash {
                return Ok(PushOutcome::RemoteChanged {
                    remote_sha256: remote_hash,
                    remote_updated_at: current.updated_at,
                });
            }
        }
        client.update(&entry.remote_id, filename, content).await?
    } else {
        client.create(filename, content, filename).await?
    };

    Ok(PushOutcome::Pushed(FileEntry {
        backend: crate::store::GIST_BACKEND.into(),
        remote_id: gist.id,
        url: gist.html_url,
        local_sha256: hash.clone(),
        remote_sha256: hash,
        last_synced: now,
        remote_updated_at: Some(gist.updated_at),
    }))
}

/// Pull remote content. Returns (content, updated FileEntry).
pub async fn pull(
    client: &GistClient,
    entry: &FileEntry,
    filename: &str,
) -> Result<(String, FileEntry)> {
    let gist = client.get(&entry.remote_id).await?;
    let content = client
        .file_content(&gist, filename)
        .await?
        .unwrap_or_default();
    let hash = sha256_hex(&content);
    let now = Utc::now();

    let updated = FileEntry {
        backend: crate::store::GIST_BACKEND.into(),
        remote_id: entry.remote_id.clone(),
        url: entry.url.clone(),
        local_sha256: hash.clone(),
        remote_sha256: hash,
        last_synced: now,
        remote_updated_at: Some(gist.updated_at),
    };

    Ok((content, updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(local: &str, remote: &str) -> FileEntry {
        FileEntry {
            backend: crate::store::GIST_BACKEND.into(),
            remote_id: "g".into(),
            url: "u".into(),
            local_sha256: local.into(),
            remote_sha256: remote.into(),
            last_synced: Utc.timestamp_opt(0, 0).unwrap(),
            remote_updated_at: None,
        }
    }

    #[test]
    fn sha256_hex_known_vector() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn local_status_not_gisted_when_no_entry() {
        assert_eq!(local_status("hello", None), SyncStatus::NotGisted);
    }

    #[test]
    fn local_status_synced_when_hashes_match() {
        let h = sha256_hex("hello");
        let e = entry(&h, &h);
        assert_eq!(local_status("hello", Some(&e)), SyncStatus::Synced);
    }

    #[test]
    fn local_status_local_newer_when_local_changed_remote_unchanged() {
        let stored = sha256_hex("v1");
        let e = entry(&stored, &stored);
        assert_eq!(local_status("v2", Some(&e)), SyncStatus::LocalNewer);
    }

    #[test]
    fn local_status_remote_newer_when_remote_diverged() {
        let local_hash = sha256_hex("v1");
        let e = entry(&local_hash, "different-remote-hash");
        assert_eq!(local_status("v1", Some(&e)), SyncStatus::RemoteNewer);
    }

    #[test]
    fn local_status_conflict_when_both_diverged() {
        let stored = sha256_hex("v1");
        let e = entry(&stored, "different-remote-hash");
        assert_eq!(local_status("v3", Some(&e)), SyncStatus::Conflict);
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gist_json(id: &str, filename: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "html_url": format!("https://gist.github.com/u/{id}"),
            "description": null,
            "public": false,
            "files": {
                filename: {
                    "filename": filename,
                    "size": content.len(),
                    "raw_url": null,
                    "content": content,
                    "truncated": false,
                }
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
        })
    }

    #[tokio::test]
    async fn push_blocks_when_remote_diverged() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "g1",
                "a.md",
                "remote v2",
            )))
            .mount(&server)
            .await;
        // No PATCH mock: a push attempt would 404 and fail the test.
        let client = gist_rs::GistClient::with_base_url("t".into(), server.uri());
        let mut stored = entry(&sha256_hex("v1"), &sha256_hex("v1"));
        stored.remote_id = "g1".into();

        let outcome = push(&client, Some(&stored), "a.md", "local v3", false)
            .await
            .unwrap();
        match outcome {
            PushOutcome::RemoteChanged { remote_sha256, .. } => {
                assert_eq!(remote_sha256, sha256_hex("remote v2"));
            }
            PushOutcome::Pushed(_) => panic!("push should have been blocked"),
        }
    }

    #[tokio::test]
    async fn push_proceeds_when_remote_unchanged() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json("g1", "a.md", "v1")))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/gists/g1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(gist_json("g1", "a.md", "local v3")),
            )
            .mount(&server)
            .await;
        let client = gist_rs::GistClient::with_base_url("t".into(), server.uri());
        let mut stored = entry(&sha256_hex("v1"), &sha256_hex("v1"));
        stored.remote_id = "g1".into();

        let outcome = push(&client, Some(&stored), "a.md", "local v3", false)
            .await
            .unwrap();
        match outcome {
            PushOutcome::Pushed(e) => {
                assert_eq!(e.local_sha256, sha256_hex("local v3"));
                assert_eq!(e.remote_sha256, e.local_sha256);
                assert!(e.remote_updated_at.is_some());
            }
            PushOutcome::RemoteChanged { .. } => panic!("push should have proceeded"),
        }
    }

    #[tokio::test]
    async fn push_force_skips_the_divergence_check() {
        let server = MockServer::start().await;
        // Only PATCH is mocked - a force push must not GET first.
        Mock::given(method("PATCH"))
            .and(path("/gists/g1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(gist_json("g1", "a.md", "local v3")),
            )
            .mount(&server)
            .await;
        let client = gist_rs::GistClient::with_base_url("t".into(), server.uri());
        let mut stored = entry(&sha256_hex("v1"), "remote-hash-we-know-diverged");
        stored.remote_id = "g1".into();

        let outcome = push(&client, Some(&stored), "a.md", "local v3", true)
            .await
            .unwrap();
        assert!(matches!(outcome, PushOutcome::Pushed(_)));
    }

    #[tokio::test]
    async fn pull_follows_raw_url_for_truncated_files() {
        let server = MockServer::start().await;
        let raw = format!("{}/raw/a.md", server.uri());
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "g1",
                "html_url": "https://gist.github.com/u/g1",
                "description": null,
                "public": false,
                "files": {
                    "a.md": {
                        "filename": "a.md",
                        "size": 2_000_000,
                        "raw_url": raw,
                        "content": "partial",
                        "truncated": true,
                    }
                },
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-06-01T00:00:00Z",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/raw/a.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("the full content"))
            .mount(&server)
            .await;
        let client = gist_rs::GistClient::with_base_url("t".into(), server.uri());
        let mut stored = entry("x", "y");
        stored.remote_id = "g1".into();

        let (content, updated) = pull(&client, &stored, "a.md").await.unwrap();
        assert_eq!(content, "the full content");
        assert_eq!(updated.remote_sha256, sha256_hex("the full content"));
    }
}
