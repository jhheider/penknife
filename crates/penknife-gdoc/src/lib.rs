//! Google Docs publish backend for penknife.
//!
//! Uses the Drive API's markdown conversion (no Docs API): create is a
//! multipart upload converted to a Doc, re-publish replaces the whole Doc,
//! and export reads back as (lossy) markdown. Auth is the OAuth device flow
//! with the non-sensitive `drive.file` scope and an on-disk token cache.

pub mod auth;
pub mod backend;
pub mod client;
pub mod error;

pub use auth::{Authenticator, Credentials, DeviceAuth, TokenSet};
pub use client::{DriveFile, GdocClient};
pub use error::GdocError;

#[cfg(test)]
mod tests {
    use super::*;
    use penknife_backend::{Backend, BackendError, BackendKind};
    use wiremock::matchers::{body_string_contains, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn creds() -> Credentials {
        Credentials {
            client_id: "cid".into(),
            client_secret: "csecret".into(),
        }
    }

    /// A token cache primed with a fresh token, so client calls skip auth.
    fn primed_auth(dir: &tempfile::TempDir, auth_base: String) -> Authenticator {
        let cache = dir.path().join("token.json");
        std::fs::write(
            &cache,
            serde_json::json!({
                "access_token": "atoken",
                "refresh_token": "rtoken",
                "expires_at": chrono::Utc::now() + chrono::Duration::hours(1),
            })
            .to_string(),
        )
        .unwrap();
        Authenticator::with_auth_base(creds(), cache, auth_base)
    }

    #[tokio::test]
    async fn device_flow_polls_until_approved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dc",
                "user_code": "ABCD-EFGH",
                "verification_url": "https://www.google.com/device",
                "interval": 0,
                "expires_in": 60,
            })))
            .mount(&server)
            .await;
        // First poll: pending. Second: token.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(428)
                    .set_body_json(serde_json::json!({"error": "authorization_pending"})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh",
                "refresh_token": "refr",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("token.json");
        let auth = Authenticator::with_auth_base(creds(), cache.clone(), server.uri());
        let flow = auth.start_device_flow().await.unwrap();
        assert_eq!(flow.user_code, "ABCD-EFGH");
        let token = auth.poll_device_flow(&flow).await.unwrap();
        assert_eq!(token.access_token, "fresh");
        // Token cache persisted, and future calls use it without the network.
        assert!(cache.exists());
        assert_eq!(auth.access_token().await.unwrap(), "fresh");
    }

    #[tokio::test]
    async fn expired_token_refreshes_and_repersists() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "renewed",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("token.json");
        std::fs::write(
            &cache,
            serde_json::json!({
                "access_token": "stale",
                "refresh_token": "rtoken",
                "expires_at": chrono::Utc::now() - chrono::Duration::hours(1),
            })
            .to_string(),
        )
        .unwrap();
        let auth = Authenticator::with_auth_base(creds(), cache.clone(), server.uri());
        assert_eq!(auth.access_token().await.unwrap(), "renewed");
        // Google omitted the refresh token; the old one must be kept.
        let saved: TokenSet =
            serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("rtoken"));
    }

    #[tokio::test]
    async fn missing_cache_means_not_authenticated() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Authenticator::with_auth_base(
            creds(),
            dir.path().join("token.json"),
            "http://unused".into(),
        );
        assert!(!auth.has_cached_token());
        let err = auth.access_token().await.unwrap_err();
        assert!(matches!(err, GdocError::NotAuthenticated));
    }

    #[tokio::test]
    async fn create_uploads_multipart_markdown_with_conversion() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/drive/v3/files"))
            .and(query_param("uploadType", "multipart"))
            .and(header("authorization", "Bearer atoken"))
            .and(body_string_contains("application/vnd.google-apps.document"))
            .and(body_string_contains("# hello"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "doc1",
                "webViewLink": "https://docs.google.com/document/d/doc1/edit",
                "modifiedTime": "2026-07-04T12:00:00Z",
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let client = GdocClient::with_api_base(primed_auth(&dir, server.uri()), server.uri());
        // Through the trait, exactly as penknife will call it.
        let backend: &dyn Backend = &client;
        assert_eq!(backend.name(), "gdoc");
        assert_eq!(backend.kind(), BackendKind::Publish);
        let r = backend.create("essay.md", "# hello", "").await.unwrap();
        assert_eq!(r.remote_id, "doc1");
        assert!(r.url.contains("docs.google.com"));
        assert!(r.revision.is_some());
    }

    #[tokio::test]
    async fn update_replaces_content_via_media_upload() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/upload/drive/v3/files/doc1"))
            .and(query_param("uploadType", "media"))
            .and(header("content-type", "text/markdown"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "doc1",
                "webViewLink": "https://docs.google.com/document/d/doc1/edit",
                "modifiedTime": "2026-07-04T13:00:00Z",
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let client = GdocClient::with_api_base(primed_auth(&dir, server.uri()), server.uri());
        let backend: &dyn Backend = &client;
        let r = backend.update("doc1", "essay.md", "# v2").await.unwrap();
        assert_eq!(r.remote_id, "doc1");
    }

    #[tokio::test]
    async fn read_exports_markdown_with_revision() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/v3/files/doc1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "doc1",
                "modifiedTime": "2026-07-04T13:00:00Z",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/drive/v3/files/doc1/export"))
            .and(query_param("mimeType", "text/markdown"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# exported"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let client = GdocClient::with_api_base(primed_auth(&dir, server.uri()), server.uri());
        let backend: &dyn Backend = &client;
        let doc = backend.read("doc1", "essay.md").await.unwrap();
        assert_eq!(doc.content, "# exported");
        assert!(doc.revision.is_some());
    }

    #[tokio::test]
    async fn missing_doc_maps_to_not_found_and_changes_unsupported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/v3/files/nope"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let client = GdocClient::with_api_base(primed_auth(&dir, server.uri()), server.uri());
        let backend: &dyn Backend = &client;
        let err = backend.read("nope", "x.md").await.unwrap_err();
        assert!(matches!(err, BackendError::NotFound(_)));
        let err = backend.changed_since(None).await.unwrap_err();
        assert!(matches!(err, BackendError::ChangesUnsupported));
    }
}
