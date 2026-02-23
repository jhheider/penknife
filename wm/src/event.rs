use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use tokio::sync::mpsc;

use crate::hydrate::HydrationProgress;

/// Messages from async tasks back to the main UI loop.
#[derive(Debug)]
pub enum AsyncEvent {
    /// Gist push completed for a file
    PushDone {
        rel_path: String,
        result: std::result::Result<crate::store::FileEntry, String>,
    },
    /// Gist pull completed
    PullDone {
        rel_path: String,
        result: std::result::Result<(String, crate::store::FileEntry), String>,
    },
    /// Hydration progress update
    HydrationUpdate(HydrationProgress),
    /// Hydration finished
    HydrationDone(std::result::Result<usize, String>),
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
    if event::poll(timeout).ok()? {
        if let Event::Key(key) = event::read().ok()? {
            return Some(key);
        }
    }
    None
}

pub type AsyncSender = mpsc::UnboundedSender<AsyncEvent>;
pub type AsyncReceiver = mpsc::UnboundedReceiver<AsyncEvent>;

pub fn async_channel() -> (AsyncSender, AsyncReceiver) {
    mpsc::unbounded_channel()
}
