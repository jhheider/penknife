//! [`penknife_backend::Backend`] implementation for Google Docs. Markdown
//! up-renders to a real Doc, which is lossy on the way back, so this is a
//! [`BackendKind::Publish`] backend: push-only in the UI.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use penknife_backend::{
    Backend, BackendError, BackendKind, RemoteChange, RemoteDoc, RemoteRef, Result,
};

use crate::client::{DriveFile, GdocClient};
use crate::error::GdocError;

fn map_err(e: GdocError) -> BackendError {
    match e {
        GdocError::Api { status: 404, .. } => BackendError::NotFound(e.to_string()),
        other => BackendError::Api(other.to_string()),
    }
}

fn to_ref(f: DriveFile) -> RemoteRef {
    RemoteRef {
        url: f.web_view_link.unwrap_or_default(),
        remote_id: f.id,
        revision: f.modified_time,
    }
}

#[async_trait]
impl Backend for GdocClient {
    fn name(&self) -> &'static str {
        "gdoc"
    }

    fn kind(&self) -> BackendKind {
        BackendKind::Publish
    }

    async fn create(&self, filename: &str, content: &str, _description: &str) -> Result<RemoteRef> {
        // The Doc's title is the filename without its extension; ".md" on a
        // Google Doc reads as clutter.
        let name = filename.trim_end_matches(".md");
        let file = self.create_doc(name, content).await.map_err(map_err)?;
        Ok(to_ref(file))
    }

    async fn read(&self, remote_id: &str, _filename: &str) -> Result<RemoteDoc> {
        let (content, revision) = self.export_markdown(remote_id).await.map_err(map_err)?;
        Ok(RemoteDoc { content, revision })
    }

    async fn update(&self, remote_id: &str, _filename: &str, content: &str) -> Result<RemoteRef> {
        let file = self.update_doc(remote_id, content).await.map_err(map_err)?;
        Ok(to_ref(file))
    }

    async fn delete(&self, remote_id: &str) -> Result<()> {
        self.delete_doc(remote_id).await.map_err(map_err)
    }

    async fn changed_since(&self, _since: Option<DateTime<Utc>>) -> Result<Vec<RemoteChange>> {
        // Drive's changes feed needs page-token state we don't keep yet;
        // callers fall back to per-document metadata reads.
        Err(BackendError::ChangesUnsupported)
    }
}
