use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
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
        result: std::result::Result<(String, crate::store::FileEntry), String>,
    },
    /// Hydration progress update
    HydrationUpdate(HydrationProgress),
    /// Hydration finished. On success, carries the (partial) updated store
    /// so the main thread can merge in the discovered mappings without
    /// clobbering concurrent writes, plus any ambiguous matches that need
    /// user resolution.
    HydrationDone(std::result::Result<HydrationDoneData, String>),
    /// Remote status check result
    StatusCheck {
        rel_path: String,
        result: std::result::Result<(crate::sync::SyncStatus, String), String>,
    },
    /// Google Doc fetch result
    GdocFetched(std::result::Result<String, String>),
}

/// Poll for a crossterm key event with the given timeout.
pub fn poll_key(timeout: Duration) -> Option<KeyEvent> {
    if event::poll(timeout).ok()?
        && let Event::Key(key) = event::read().ok()?
    {
        return Some(key);
    }
    None
}

pub type AsyncSender = mpsc::UnboundedSender<AsyncEvent>;
pub type AsyncReceiver = mpsc::UnboundedReceiver<AsyncEvent>;

pub fn async_channel() -> (AsyncSender, AsyncReceiver) {
    mpsc::unbounded_channel()
}
