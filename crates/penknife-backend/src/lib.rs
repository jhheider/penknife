//! The backend contract penknife publishes documents through.
//!
//! A backend is one remote service that can hold a copy of a local document
//! (GitHub Gists today; the seam is here for more). The trait is deliberately
//! small; it mirrors exactly what penknife's sync engine consumes (create,
//! read, update, delete, and an optional changed-since feed for cheap
//! polling), so one implementation unlocks the whole UI.
//!
//! Every backend is a *sync* backend: content round-trips losslessly, so
//! pulling remote content back over the local file is safe. (An earlier
//! design also modeled lossy "publish-only" backends; that was cut along
//! with the Google Docs backend, since a lossy remote can't participate in
//! the drift model honestly.)

use async_trait::async_trait;
use chrono::{DateTime, Utc};

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
#[non_exhaustive]
pub enum BackendError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("this backend has no change feed")]
    ChangesUnsupported,
    #[error("{0}")]
    Message(String),
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
    fn error_display_is_lowercase_and_punctuation_free() {
        // Convention (C-GOOD-ERR): compose cleanly when wrapped.
        assert_eq!(
            BackendError::NotFound("x".into()).to_string(),
            "not found: x"
        );
        assert_eq!(
            BackendError::ChangesUnsupported.to_string(),
            "this backend has no change feed"
        );
        assert_eq!(BackendError::Message("boom".into()).to_string(), "boom");
    }

    #[test]
    fn value_types_carry_their_fields() {
        let now = chrono::Utc::now();
        let r = RemoteRef {
            remote_id: "id".into(),
            url: "u".into(),
            revision: Some(now),
        };
        assert_eq!(r.remote_id, "id");
        let d = RemoteDoc {
            content: "body".into(),
            revision: None,
        };
        assert_eq!(d.content, "body");
        let c = RemoteChange {
            remote_id: "id".into(),
            revision: now,
        };
        assert_eq!(c.revision, now);
    }
}
