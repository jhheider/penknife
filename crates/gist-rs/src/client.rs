use std::collections::HashMap;
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, Response, StatusCode};

use crate::error::{GistError, Result};
use crate::types::*;

const API_BASE: &str = "https://api.github.com";
const MAX_RETRIES: u32 = 3;
/// Files larger than this can't be fetched via `raw_url`; GitHub requires
/// cloning the gist's git repo.
const RAW_FETCH_LIMIT: u64 = 10 * 1024 * 1024;
/// Per-request timeout. Without one, reqwest waits forever and a hung
/// connection silently strands the operation.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// One page of gists plus pagination metadata.
#[derive(Debug)]
pub struct GistPage {
    pub gists: Vec<Gist>,
    /// Whether the response's Link header indicates more pages exist.
    pub has_next: bool,
}

pub struct GistClient {
    http: Client,
    token: String,
    base_url: String,
}

impl GistClient {
    pub fn new(token: String) -> Self {
        Self::with_base_url(token, API_BASE.to_string())
    }

    /// Construct a client against a non-default API base URL (mock servers
    /// in tests, GitHub Enterprise).
    pub fn with_base_url(token: String, base_url: String) -> Self {
        Self {
            http: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            token,
            base_url,
        }
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "writings-manager")
            .header(ACCEPT, "application/vnd.github+json")
    }

    /// Send a GET request with retries on transient failures and rate-limit awareness.
    /// Only safe for idempotent requests — do not use for POST/PATCH/DELETE.
    async fn get_with_retry(&self, url: &str) -> Result<Response> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let resp_result = self.request(reqwest::Method::GET, url).send().await;
            match resp_result {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp);
                    }
                    if let Some(wait) = rate_limit_wait(&resp)
                        && attempt < MAX_RETRIES
                    {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if status.is_server_error() && attempt < MAX_RETRIES {
                        tokio::time::sleep(backoff(attempt)).await;
                        continue;
                    }
                    let msg = resp.text().await.unwrap_or_default();
                    return Err(GistError::Api {
                        status: status.as_u16(),
                        message: msg,
                    });
                }
                Err(e) if attempt < MAX_RETRIES => {
                    // Network-level failure — retry with backoff.
                    tokio::time::sleep(backoff(attempt)).await;
                    let _ = e;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// List a single page of gists (100 per page) along with pagination metadata.
    pub async fn list_page(&self, page: u32) -> Result<GistPage> {
        let url = format!("{}/gists?per_page=100&page={page}", self.base_url);
        let resp = self.get_with_retry(&url).await?;
        let has_next = resp
            .headers()
            .get("link")
            .and_then(|h| h.to_str().ok())
            .map(link_header_has_next)
            .unwrap_or(false);
        let gists: Vec<Gist> = resp.json().await?;
        Ok(GistPage { gists, has_next })
    }

    /// List all gists for the authenticated user, paginating automatically.
    pub async fn list_all(&self) -> Result<Vec<Gist>> {
        let mut all = Vec::new();
        let mut page = 1u32;

        loop {
            let result = self.list_page(page).await?;
            all.extend(result.gists);
            if !result.has_next {
                break;
            }
            page += 1;
        }

        Ok(all)
    }

    /// Get a single gist by ID (includes file content).
    pub async fn get(&self, id: &str) -> Result<Gist> {
        let url = format!("{}/gists/{id}", self.base_url);
        let resp = self.get_with_retry(&url).await?;
        Ok(resp.json().await?)
    }

    /// Full content of one file in a gist, following `raw_url` when the API
    /// response omitted or truncated the body (the Gists API truncates file
    /// content at 1MB; list responses omit it entirely). Returns `Ok(None)`
    /// if the gist has no file by that name. Files beyond the 10MB raw-fetch
    /// limit return `GistError::TooLarge` rather than silently-partial data.
    pub async fn file_content(&self, gist: &Gist, filename: &str) -> Result<Option<String>> {
        let Some(file) = gist.files.get(filename) else {
            return Ok(None);
        };
        if let Some(content) = &file.content
            && !file.truncated
        {
            return Ok(Some(content.clone()));
        }
        if file.size > RAW_FETCH_LIMIT {
            return Err(GistError::TooLarge {
                filename: filename.to_string(),
                size: file.size,
            });
        }
        let Some(raw_url) = file.raw_url.as_deref() else {
            return Err(GistError::Api {
                status: 0,
                message: format!("file {filename} has no content and no raw_url"),
            });
        };
        let resp = self.get_with_retry(raw_url).await?;
        Ok(Some(resp.text().await?))
    }

    /// Create a new private gist with a single file.
    pub async fn create(&self, filename: &str, content: &str, description: &str) -> Result<Gist> {
        let mut files = HashMap::new();
        files.insert(
            filename.to_string(),
            GistFileContent {
                content: content.to_string(),
            },
        );
        let body = CreateGistRequest {
            description: description.to_string(),
            public: false,
            files,
        };
        let url = format!("{}/gists", self.base_url);
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&body)
            .send()
            .await?;
        check_response(resp).await
    }

    /// Update an existing gist's file content.
    pub async fn update(&self, id: &str, filename: &str, content: &str) -> Result<Gist> {
        let mut files = HashMap::new();
        files.insert(
            filename.to_string(),
            GistFileContent {
                content: content.to_string(),
            },
        );
        let body = UpdateGistRequest {
            description: None,
            files,
        };
        let url = format!("{}/gists/{id}", self.base_url);
        let resp = self
            .request(reqwest::Method::PATCH, &url)
            .json(&body)
            .send()
            .await?;
        check_response(resp).await
    }

    /// Rename a file within a gist. GitHub's PATCH /gists/:id endpoint
    /// accepts a `{"files": {"<old>": {"filename": "<new>"}}}` shape — note
    /// the old name is the map *key*, the new name is in the value.
    pub async fn rename_file(&self, id: &str, old_name: &str, new_name: &str) -> Result<Gist> {
        let body = serde_json::json!({
            "files": {
                old_name: { "filename": new_name }
            }
        });
        let url = format!("{}/gists/{id}", self.base_url);
        let resp = self
            .request(reqwest::Method::PATCH, &url)
            .json(&body)
            .send()
            .await?;
        check_response(resp).await
    }

    /// Delete a gist.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let url = format!("{}/gists/{id}", self.base_url);
        let resp = self.request(reqwest::Method::DELETE, &url).send().await?;
        let status = resp.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(());
        }
        if !status.is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(GistError::Api {
                status: status.as_u16(),
                message: msg,
            });
        }
        Ok(())
    }
}

async fn check_response(resp: reqwest::Response) -> Result<Gist> {
    let status = resp.status();
    if !status.is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(GistError::Api {
            status: status.as_u16(),
            message: msg,
        });
    }
    Ok(resp.json().await?)
}

/// Compute exponential backoff delay for the given attempt number (1-based).
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(1u64 << attempt.min(6)))
}

/// Parse a GitHub `Link` header and return true if it contains a `rel="next"`.
fn link_header_has_next(link_header: &str) -> bool {
    link_header
        .split(',')
        .any(|part| part.contains("rel=\"next\""))
}

/// Inspect a response for rate-limit signals and return the duration to wait
/// before retrying, if any. Honors `Retry-After` (429) and
/// `X-RateLimit-Remaining: 0` + `X-RateLimit-Reset`.
fn rate_limit_wait(resp: &Response) -> Option<Duration> {
    let status = resp.status();
    let headers = resp.headers();

    // Explicit Retry-After header (in seconds), commonly on 429.
    if let Some(v) = headers
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        return Some(Duration::from_secs(v.min(60)));
    }

    // Primary rate limit exhausted: 403/429 with X-RateLimit-Remaining: 0
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    if (status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS)
        && remaining == Some(0)
        && let Some(reset) = headers
            .get("x-ratelimit-reset")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
    {
        let now = chrono::Utc::now().timestamp();
        let wait = (reset - now).clamp(0, 60) as u64;
        return Some(Duration::from_secs(wait));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_header_detects_next() {
        let h = "<https://api.github.com/gists?page=2>; rel=\"next\", \
                 <https://api.github.com/gists?page=10>; rel=\"last\"";
        assert!(link_header_has_next(h));
    }

    #[test]
    fn link_header_no_next_on_last_page() {
        let h = "<https://api.github.com/gists?page=1>; rel=\"first\", \
                 <https://api.github.com/gists?page=9>; rel=\"prev\"";
        assert!(!link_header_has_next(h));
    }

    #[test]
    fn link_header_empty_means_no_next() {
        assert!(!link_header_has_next(""));
    }

    #[test]
    fn backoff_grows_with_attempt() {
        assert!(backoff(2) > backoff(1));
        assert!(backoff(3) > backoff(2));
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gist_json(
        id: &str,
        filename: &str,
        content: Option<&str>,
        truncated: bool,
        size: u64,
        raw_url: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "html_url": format!("https://gist.github.com/u/{id}"),
            "description": null,
            "public": false,
            "files": {
                filename: {
                    "filename": filename,
                    "size": size,
                    "raw_url": raw_url,
                    "content": content,
                    "truncated": truncated,
                }
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
        })
    }

    fn gist_from_json(v: serde_json::Value) -> Gist {
        serde_json::from_value(v).unwrap()
    }

    #[tokio::test]
    async fn file_content_returns_inline_content_without_fetching() {
        // Unreachable base URL — any HTTP request would error out.
        let client = GistClient::with_base_url("t".into(), "http://127.0.0.1:1".into());
        let gist = gist_from_json(gist_json("g1", "a.md", Some("hello"), false, 5, None));
        let got = client.file_content(&gist, "a.md").await.unwrap();
        assert_eq!(got.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn file_content_returns_none_for_missing_file() {
        let client = GistClient::with_base_url("t".into(), "http://127.0.0.1:1".into());
        let gist = gist_from_json(gist_json("g1", "a.md", Some("hello"), false, 5, None));
        assert!(
            client
                .file_content(&gist, "other.md")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn file_content_follows_raw_url_when_truncated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/raw/a.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("full body"))
            .mount(&server)
            .await;
        let client = GistClient::with_base_url("t".into(), server.uri());
        let raw = format!("{}/raw/a.md", server.uri());
        let gist = gist_from_json(gist_json(
            "g1",
            "a.md",
            Some("trunc"),
            true,
            2_000_000,
            Some(&raw),
        ));
        let got = client.file_content(&gist, "a.md").await.unwrap();
        assert_eq!(got.as_deref(), Some("full body"));
    }

    #[tokio::test]
    async fn file_content_rejects_files_beyond_raw_fetch_limit() {
        let client = GistClient::with_base_url("t".into(), "http://127.0.0.1:1".into());
        let gist = gist_from_json(gist_json(
            "g1",
            "a.md",
            None,
            true,
            RAW_FETCH_LIMIT + 1,
            Some("http://127.0.0.1:1/raw"),
        ));
        let err = client.file_content(&gist, "a.md").await.unwrap_err();
        assert!(matches!(err, GistError::TooLarge { size, .. } if size == RAW_FETCH_LIMIT + 1));
    }

    #[tokio::test]
    async fn get_retries_server_errors_then_succeeds() {
        let server = MockServer::start().await;
        // First request 500s; the retry (mounted-mock now exhausted) gets 200.
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "g1",
                "a.md",
                Some("x"),
                false,
                1,
                None,
            )))
            .mount(&server)
            .await;
        let client = GistClient::with_base_url("t".into(), server.uri());
        let gist = client.get("g1").await.unwrap();
        assert_eq!(gist.id, "g1");
    }
}
