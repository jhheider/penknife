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

/// Full status check including remote fetch.
pub async fn full_status(
    client: &GistClient,
    local_content: &str,
    entry: &FileEntry,
    filename: &str,
) -> Result<(SyncStatus, String)> {
    let gist = client.get(&entry.gist_id).await?;
    let remote_content = gist
        .files
        .get(filename)
        .and_then(|f| f.content.as_deref())
        .unwrap_or("");
    let remote_hash = sha256_hex(remote_content);
    let local_hash = sha256_hex(local_content);

    let local_changed = local_hash != entry.local_sha256;
    let remote_changed = remote_hash != entry.remote_sha256;

    let status = match (local_changed, remote_changed) {
        (false, false) => SyncStatus::Synced,
        (true, false) => SyncStatus::LocalNewer,
        (false, true) => SyncStatus::RemoteNewer,
        (true, true) => SyncStatus::Conflict,
    };

    Ok((status, remote_content.to_string()))
}

/// Push local content to gist (create or update). Returns updated FileEntry.
pub async fn push(
    client: &GistClient,
    existing: Option<&FileEntry>,
    rel_path: &str,
    filename: &str,
    content: &str,
) -> Result<FileEntry> {
    let hash = sha256_hex(content);
    let now = Utc::now();

    let gist = if let Some(entry) = existing {
        client.update(&entry.gist_id, filename, content).await?
    } else {
        client.create(filename, content, rel_path).await?
    };

    Ok(FileEntry {
        gist_id: gist.id,
        url: gist.html_url,
        local_sha256: hash.clone(),
        remote_sha256: hash,
        last_synced: now,
    })
}

/// Pull remote content. Returns (content, updated FileEntry).
pub async fn pull(
    client: &GistClient,
    entry: &FileEntry,
    filename: &str,
) -> Result<(String, FileEntry)> {
    let gist = client.get(&entry.gist_id).await?;
    let content = gist
        .files
        .get(filename)
        .and_then(|f| f.content.clone())
        .unwrap_or_default();
    let hash = sha256_hex(&content);
    let now = Utc::now();

    let updated = FileEntry {
        gist_id: entry.gist_id.clone(),
        url: entry.url.clone(),
        local_sha256: hash.clone(),
        remote_sha256: hash,
        last_synced: now,
    };

    Ok((content, updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(local: &str, remote: &str) -> FileEntry {
        FileEntry {
            gist_id: "g".into(),
            url: "u".into(),
            local_sha256: local.into(),
            remote_sha256: remote.into(),
            last_synced: Utc.timestamp_opt(0, 0).unwrap(),
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
}
