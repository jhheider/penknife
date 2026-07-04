//! Drive API calls for the markdown-conversion publish path.
//!
//! Create is a multipart/related upload whose metadata half sets the target
//! mimeType to a Google Doc; Drive converts the markdown media half on the
//! way in. Re-publish is a media upload against the existing file, which
//! replaces the whole document: exactly push semantics, so the "limitation"
//! costs nothing. Read exports back as markdown (lossy; images don't
//! round-trip), which is fine for diff display but is never offered as a
//! destructive pull because this backend is [`BackendKind::Publish`].

use chrono::{DateTime, Utc};

use crate::auth::Authenticator;
use crate::error::{GdocError, Result};

const DEFAULT_API_BASE: &str = "https://www.googleapis.com";
const DOC_MIME: &str = "application/vnd.google-apps.document";
const MARKDOWN_MIME: &str = "text/markdown";
/// multipart/related boundary. Fixed and collision-improbable; a document
/// that contains this exact line only breaks its own upload.
const BOUNDARY: &str = "penknife-gdoc-3c2f9d41a7b8";

/// Metadata Drive returns about a file, trimmed to what we use.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    #[serde(default)]
    pub web_view_link: Option<String>,
    #[serde(default)]
    pub modified_time: Option<DateTime<Utc>>,
}

pub struct GdocClient {
    auth: Authenticator,
    api_base: String,
    http: reqwest::Client,
}

impl GdocClient {
    pub fn new(auth: Authenticator) -> Self {
        Self::with_api_base(auth, DEFAULT_API_BASE.to_string())
    }

    /// Construct against a non-default API base URL (mock servers in tests).
    pub fn with_api_base(auth: Authenticator, api_base: String) -> Self {
        Self {
            auth,
            api_base,
            http: reqwest::Client::new(),
        }
    }

    pub fn auth(&self) -> &Authenticator {
        &self.auth
    }

    /// Create a Google Doc from markdown. Returns the new file's metadata,
    /// including its user-facing URL.
    pub async fn create_doc(&self, name: &str, markdown: &str) -> Result<DriveFile> {
        let token = self.auth.access_token().await?;
        let metadata = serde_json::json!({ "name": name, "mimeType": DOC_MIME });
        let body = format!(
            "--{BOUNDARY}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{BOUNDARY}\r\nContent-Type: {MARKDOWN_MIME}; charset=UTF-8\r\n\r\n{markdown}\r\n--{BOUNDARY}--"
        );
        let resp = self
            .http
            .post(format!(
                "{}/upload/drive/v3/files?uploadType=multipart&fields=id,webViewLink,modifiedTime",
                self.api_base
            ))
            .bearer_auth(&token)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={BOUNDARY}"),
            )
            .body(body)
            .send()
            .await?;
        Self::parse_file(resp).await
    }

    /// Replace an existing Doc's content with fresh markdown. Drive converts
    /// on the way in because the file's mimeType stays a Google Doc.
    pub async fn update_doc(&self, file_id: &str, markdown: &str) -> Result<DriveFile> {
        let token = self.auth.access_token().await?;
        let resp = self
            .http
            .patch(format!(
                "{}/upload/drive/v3/files/{file_id}?uploadType=media&fields=id,webViewLink,modifiedTime",
                self.api_base
            ))
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_TYPE, MARKDOWN_MIME)
            .body(markdown.to_string())
            .send()
            .await?;
        Self::parse_file(resp).await
    }

    /// Export a Doc back as markdown. Lossy (images become data URLs or are
    /// dropped); used for display, never for a silent pull.
    pub async fn export_markdown(&self, file_id: &str) -> Result<(String, Option<DateTime<Utc>>)> {
        let token = self.auth.access_token().await?;
        // Revision comes from metadata; the export endpoint has no headers
        // worth trusting for it.
        let meta = self.get_metadata(file_id).await?;
        let resp = self
            .http
            .get(format!(
                "{}/drive/v3/files/{file_id}/export?mimeType={MARKDOWN_MIME}",
                self.api_base
            ))
            .bearer_auth(&token)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GdocError::Api {
                status: status.as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        Ok((resp.text().await?, meta.modified_time))
    }

    pub async fn get_metadata(&self, file_id: &str) -> Result<DriveFile> {
        let token = self.auth.access_token().await?;
        let resp = self
            .http
            .get(format!(
                "{}/drive/v3/files/{file_id}?fields=id,webViewLink,modifiedTime",
                self.api_base
            ))
            .bearer_auth(&token)
            .send()
            .await?;
        Self::parse_file(resp).await
    }

    pub async fn delete_doc(&self, file_id: &str) -> Result<()> {
        let token = self.auth.access_token().await?;
        let resp = self
            .http
            .delete(format!("{}/drive/v3/files/{file_id}", self.api_base))
            .bearer_auth(&token)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GdocError::Api {
                status: status.as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(())
    }

    async fn parse_file(resp: reqwest::Response) -> Result<DriveFile> {
        let status = resp.status();
        if !status.is_success() {
            return Err(GdocError::Api {
                status: status.as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(resp.json().await?)
    }
}
