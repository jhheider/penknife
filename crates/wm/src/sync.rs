use sha2::{Digest, Sha256};

use chrono::Utc;
use gist_rs::GistClient;
use ratatui::style::Color;

use crate::error::Result;
use crate::store::{FileEntry, Store};

/// SHA-256 hash of content as hex string.
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
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
        match self {
            Self::Synced => "✅",
            Self::LocalNewer => "⬆️",
            Self::RemoteNewer => "⬇️",
            Self::Conflict => "❗",
            Self::NotGisted => "⚪",
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
    store: &Store,
    rel_path: &str,
    filename: &str,
    content: &str,
) -> Result<FileEntry> {
    let hash = sha256_hex(content);
    let now = Utc::now();

    let gist = if let Some(entry) = store.get(rel_path) {
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
