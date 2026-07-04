//! The backend contract penknife publishes documents through.
//!
//! A backend is one remote service that can hold a copy of a local document:
//! GitHub Gists, Google Docs, Notion, and so on. The trait is deliberately
//! small; it mirrors exactly what penknife's sync engine consumes (create,
//! read, update, delete, and an optional changed-since feed for cheap
//! polling), so one implementation unlocks the whole UI.
//!
//! Backends declare themselves [`BackendKind::Sync`] or
//! [`BackendKind::Publish`]. Sync backends round-trip content losslessly, so
//! pulling remote content over the local file is safe. Publish backends
//! up-render on the way out (markdown → a Google Doc) and cannot faithfully
//! come back; penknife treats their remote edits as "diverged, view in
//! browser" rather than offering a destructive pull.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// How faithfully content survives a round trip through this backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Lossless round-trip: what you push is byte-for-byte what you can
    /// read back. Pull is safe.
    Sync,
    /// Lossy up-render: push replaces the remote wholesale, and reading
    /// back may lose formatting. Push-only in the UI.
    Publish,
}

/// A reference to a document held by a backend, as returned by mutations.
#[derive(Debug, Clone)]
pub struct RemoteRef {
    /// The backend's identifier for the document (gist ID, Doc ID, ...).
    pub remote_id: String,
    /// A user-facing URL for the document.
    pub url: String,
    /// The backend's revision timestamp after the mutation, if it has one.
    pub revision: Option<DateTime<Utc>>,
}

/// A document read back from a backend.
#[derive(Debug, Clone)]
pub struct RemoteDoc {
    pub content: String,
    /// The backend's revision timestamp for this content, if it has one.
    pub revision: Option<DateTime<Utc>>,
}

/// One entry in a backend's change feed.
#[derive(Debug, Clone)]
pub struct RemoteChange {
    pub remote_id: String,
    pub revision: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("this backend has no change feed")]
    ChangesUnsupported,
    #[error("{0}")]
    Api(String),
}

pub type Result<T> = std::result::Result<T, BackendError>;

/// One remote service that can hold copies of local documents.
///
/// `async_trait` keeps the trait object-safe: penknife holds backends as
/// `Arc<dyn Backend>` and dispatches by the `backend` field on each stored
/// copy.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Stable machine name, recorded in the store per copy (e.g. "gist").
    fn name(&self) -> &'static str;

    /// Whether round-trips are lossless (sync) or push-only (publish).
    fn kind(&self) -> BackendKind;

    /// Publish a new document. `description` may be ignored by backends
    /// without such a field.
    async fn create(&self, filename: &str, content: &str, description: &str) -> Result<RemoteRef>;

    /// Read a document's current content. `filename` selects the file for
    /// container-shaped backends (a gist holds several files); single-doc
    /// backends may ignore it.
    async fn read(&self, remote_id: &str, filename: &str) -> Result<RemoteDoc>;

    /// Replace a document's content.
    async fn update(&self, remote_id: &str, filename: &str, content: &str) -> Result<RemoteRef>;

    /// Delete a document.
    async fn delete(&self, remote_id: &str) -> Result<()>;

    /// Documents changed after `since` (all documents when `None`). Powers
    /// cheap polling and incremental hydration. Backends without a change
    /// feed return [`BackendError::ChangesUnsupported`]; callers fall back
    /// to per-document reads.
    async fn changed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<RemoteChange>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait must stay object-safe: penknife stores `Arc<dyn Backend>`.
    #[test]
    fn backend_is_object_safe() {
        fn _takes_dyn(_: &dyn Backend) {}
    }

    #[test]
    fn kinds_are_distinguishable() {
        assert_ne!(BackendKind::Sync, BackendKind::Publish);
    }
}
