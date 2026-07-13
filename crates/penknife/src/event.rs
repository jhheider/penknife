use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, MouseEvent};
use tokio::sync::mpsc;

use crate::git::GitStatus;
use crate::hydrate::{AmbiguousMatch, HydrationProgress};
use crate::scanner::ScannedFile;
use crate::store::Store;
use crate::sync::SyncStatus;

/// Payload of a successful gist-import fetch: the file's markdown content,
/// the gist's filename (prefills the save-as prompt), and the ready-made
/// store entry (import means local == remote, i.e. born synced).
#[derive(Debug)]
pub struct GistImportData {
    pub content: String,
    pub filename: String,
    pub entry: crate::store::FileEntry,
}

#[derive(Debug)]
pub struct HydrationDoneData {
    pub matched: usize,
    pub ambiguous: Vec<AmbiguousMatch>,
    pub store: Box<Store>,
    /// The root this walk covered - the per-root hydration cursor is keyed by it.
    pub root: PathBuf,
    /// The timestamp to record as `root`'s new hydration cursor - captured
    /// when the walk began, so gists created mid-walk are caught on the next
    /// pass rather than skipped.
    pub new_cursor: chrono::DateTime<chrono::Utc>,
}

/// Messages from async tasks back to the main UI loop.
#[derive(Debug)]
pub enum AsyncEvent {
    /// Gist push completed for a file
    PushDone {
        root: PathBuf,
        rel_path: String,
        result: std::result::Result<crate::store::FileEntry, String>,
    },
    /// Gist pull completed
    PullDone {
        root: PathBuf,
        rel_path: String,
        /// SHA-256 of the local file when the pull was initiated. If the
        /// on-disk content no longer matches by the time this event lands
        /// (user edited mid-pull), the write is refused.
        expected_local_sha256: String,
        result: std::result::Result<(String, crate::store::FileEntry), String>,
    },
    /// Push refused because the remote changed since the last sync.
    /// Carries the freshly observed remote state so the store can record
    /// the divergence and the UI can offer a force-push.
    PushBlocked {
        root: PathBuf,
        rel_path: String,
        remote_sha256: String,
        remote_updated_at: chrono::DateTime<chrono::Utc>,
    },
    /// Progress for a bulk remote check (`f`).
    /// Bulk remote check finished. `started` timestamps the check so stale
    /// results don't clobber entries that synced while it ran.
    RemoteCheckDone {
        root: PathBuf,
        started: chrono::DateTime<chrono::Utc>,
        result: std::result::Result<crate::remote::RemoteCheckOutcome, String>,
    },
    /// Hydration progress update
    HydrationUpdate(HydrationProgress),
    /// Hydration finished. On success, carries the (partial) updated store
    /// so the main thread can merge in the discovered mappings without
    /// clobbering concurrent writes, plus any ambiguous matches that need
    /// user resolution.
    HydrationDone(std::result::Result<HydrationDoneData, String>),
    /// Remote status check result (from the diff view's fetch). Carries the
    /// observed remote state so it can be written back to the store - a
    /// diff is also a remote check.
    StatusCheck {
        root: PathBuf,
        rel_path: String,
        /// When the fetch began; write-back is skipped if the entry synced
        /// after this (the sync result is the newer truth).
        started: chrono::DateTime<chrono::Utc>,
        result: std::result::Result<crate::sync::FullStatus, String>,
    },
    /// Google Doc fetch result
    GdocFetched(std::result::Result<String, String>),
    /// Gist import fetch result: content plus the mapping to record once the
    /// file is saved locally.
    GistImportFetched(std::result::Result<GistImportData, String>),
    /// Remote gist deletion completed
    DeleteDone {
        root: PathBuf,
        rel_path: String,
        result: std::result::Result<(), String>,
    },
    /// Manual link (`L`) completed: a gist was fetched and reconciled against
    /// the selected local file. On success carries the store entry to record.
    LinkDone {
        root: PathBuf,
        rel_path: String,
        result: std::result::Result<crate::store::FileEntry, String>,
    },
    /// Remote gist filename update (companion to a local rename) completed.
    /// `rel_path` here is the new rel_path (for the status message);
    /// the local store + filesystem are already updated by the time this
    /// event fires, so we don't need the root.
    RenameRemoteDone {
        rel_path: String,
        result: std::result::Result<(), String>,
    },
    /// An off-thread refresh (directory scan + per-file sync-status cache +
    /// `git status`) finished. Everything here was computed on a blocking-pool
    /// thread so the render loop never stalls on the walk, the file reads, or
    /// the git shell-out; the main loop just adopts the result.
    RefreshDone {
        /// Refresh sequence number, compared against the app's current
        /// generation on apply so a superseded (older) refresh is dropped.
        generation: u64,
        /// The root this refresh scanned; dropped if the active root changed
        /// while it ran.
        root: PathBuf,
        files: Vec<ScannedFile>,
        status_cache: HashMap<String, SyncStatus>,
        git_repo_root: Option<PathBuf>,
        git_statuses: HashMap<String, GitStatus>,
        /// If set, jump the tree selection to this rel_path after applying
        /// (used after a rename so the moved file stays selected).
        select: Option<String>,
        /// If set, the result is discarded unless the file set actually
        /// differs from what's shown - the periodic local sweep uses this so
        /// idle browsing doesn't rebuild the tree (or reset preview scroll)
        /// every interval.
        only_if_changed: bool,
        /// Status-bar message to show once the refresh lands, applied *after*
        /// the dashboard/status refresh so an operation's own "Done" line wins
        /// over the recomputed default.
        done_message: Option<String>,
    },
}

/// User-input events from the terminal that the app cares about.
#[derive(Debug)]
pub enum UiEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

/// Poll for a crossterm UI event (key or mouse) with the given timeout.
pub fn poll_event(timeout: Duration) -> Option<UiEvent> {
    if !event::poll(timeout).ok()? {
        return None;
    }
    match event::read().ok()? {
        Event::Key(k) => Some(UiEvent::Key(k)),
        Event::Mouse(m) => Some(UiEvent::Mouse(m)),
        _ => None,
    }
}

pub type AsyncSender = mpsc::UnboundedSender<AsyncEvent>;
pub type AsyncReceiver = mpsc::UnboundedReceiver<AsyncEvent>;

pub fn async_channel() -> (AsyncSender, AsyncReceiver) {
    mpsc::unbounded_channel()
}
