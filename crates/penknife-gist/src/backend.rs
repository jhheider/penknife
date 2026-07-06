//! [`penknife_backend::Backend`] implementation for GitHub Gists: the
//! founding backend. Gists round-trip text losslessly, so pull is safe.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use penknife_backend::{Backend, BackendError, RemoteChange, RemoteDoc, RemoteRef, Result};

use crate::client::GistClient;
use crate::error::GistError;

fn map_err(e: GistError) -> BackendError {
    match e {
        GistError::Api { status: 404, .. } => BackendError::NotFound(e.to_string()),
        other => BackendError::Api(other.to_string()),
    }
}

#[async_trait]
impl Backend for GistClient {
    fn name(&self) -> &'static str {
        "gist"
    }

    async fn create(&self, filename: &str, content: &str, description: &str) -> Result<RemoteRef> {
        let gist = GistClient::create(self, filename, content, description)
            .await
            .map_err(map_err)?;
        Ok(RemoteRef {
            remote_id: gist.id,
            url: gist.html_url,
            revision: Some(gist.updated_at),
        })
    }

    async fn read(&self, remote_id: &str, filename: &str) -> Result<RemoteDoc> {
        let gist = GistClient::get(self, remote_id).await.map_err(map_err)?;
        let content = self
            .file_content(&gist, filename)
            .await
            .map_err(map_err)?
            .ok_or_else(|| BackendError::NotFound(format!("{remote_id} has no {filename}")))?;
        Ok(RemoteDoc {
            content,
            revision: Some(gist.updated_at),
        })
    }

    async fn update(&self, remote_id: &str, filename: &str, content: &str) -> Result<RemoteRef> {
        let gist = GistClient::update(self, remote_id, filename, content)
            .await
            .map_err(map_err)?;
        Ok(RemoteRef {
            remote_id: gist.id,
            url: gist.html_url,
            revision: Some(gist.updated_at),
        })
    }

    async fn delete(&self, remote_id: &str) -> Result<()> {
        GistClient::delete(self, remote_id).await.map_err(map_err)
    }

    async fn changed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<RemoteChange>> {
        let mut out = Vec::new();
        let mut page = 1u32;
        loop {
            let result = self.list_page_since(page, since).await.map_err(map_err)?;
            out.extend(result.gists.into_iter().map(|g| RemoteChange {
                remote_id: g.id,
                revision: g.updated_at,
            }));
            if !result.has_next {
                break;
            }
            page += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gist_json(id: &str, filename: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "html_url": format!("https://gist.github.com/u/{id}"),
            "description": "d",
            "public": false,
            "files": {
                filename: {
                    "filename": filename,
                    "raw_url": format!("https://x/{id}/{filename}"),
                    "size": content.len(),
                    "truncated": false,
                    "content": content,
                }
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
        })
    }

    #[tokio::test]
    async fn read_roundtrips_through_the_trait() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists/abc"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(gist_json("abc", "post.md", "hello")),
            )
            .mount(&server)
            .await;

        let client = GistClient::with_base_url("t".into(), server.uri());
        // Dispatch through the trait object, exactly as penknife will.
        let backend: &dyn Backend = &client;
        assert_eq!(backend.name(), "gist");
        let doc = backend.read("abc", "post.md").await.unwrap();
        assert_eq!(doc.content, "hello");
        assert!(doc.revision.is_some());
    }

    #[tokio::test]
    async fn missing_gist_maps_to_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists/nope"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
            .mount(&server)
            .await;

        let client = GistClient::with_base_url("t".into(), server.uri());
        let backend: &dyn Backend = &client;
        let err = backend.read("nope", "x.md").await.unwrap_err();
        assert!(matches!(err, BackendError::NotFound(_)));
    }

    #[tokio::test]
    async fn changed_since_pages_through_the_feed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([gist_json("g1", "a.md", "x")])),
            )
            .mount(&server)
            .await;

        let client = GistClient::with_base_url("t".into(), server.uri());
        let backend: &dyn Backend = &client;
        let changes = backend.changed_since(None).await.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].remote_id, "g1");
    }
}
