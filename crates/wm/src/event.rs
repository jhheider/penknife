use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, MouseEvent};
use tokio::sync::mpsc;

use crate::hydrate::{AmbiguousMatch, HydrationProgress};
use crate::store::Store;

#[derive(Debug)]
pub struct HydrationDoneData {
    pub matched: usize,
    pub ambiguous: Vec<AmbiguousMatch>,
    pub store: Box<Store>,
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
    RemoteCheckProgress { done: usize, total: usize },
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
    /// observed remote state so it can be written back to the store — a
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
    /// Remote gist deletion completed
    DeleteDone {
        root: PathBuf,
        rel_path: String,
        result: std::result::Result<(), String>,
    },
    /// Remote gist filename update (companion to a local rename) completed.
    /// `rel_path` here is the new rel_path (for the status message);
    /// the local store + filesystem are already updated by the time this
    /// event fires, so we don't need the root.
    RenameRemoteDone {
        rel_path: String,
        result: std::result::Result<(), String>,
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
