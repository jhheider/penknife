use std::collections::HashMap;
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, Response, StatusCode};

use crate::error::{GistError, Result};
use crate::types::*;

const API_BASE: &str = "https://api.github.com";
const MAX_RETRIES: u32 = 3;

pub struct GistClient {
    http: Client,
    token: String,
}

impl GistClient {
    pub fn new(token: String) -> Self {
        Self {
            http: Client::new(),
            token,
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

    /// List a single page of gists (100 per page). Returns empty vec when no more pages.
    pub async fn list_page(&self, page: u32) -> Result<Vec<Gist>> {
        let url = format!("{API_BASE}/gists?per_page=100&page={page}");
        let resp = self.get_with_retry(&url).await?;
        Ok(resp.json().await?)
    }

    /// List all gists for the authenticated user, paginating automatically.
    pub async fn list_all(&self) -> Result<Vec<Gist>> {
        let mut all = Vec::new();
        let mut page = 1u32;

        loop {
            let gists = self.list_page(page).await?;
            if gists.is_empty() {
                break;
            }
            all.extend(gists);
            page += 1;
        }

        Ok(all)
    }

    /// Get a single gist by ID (includes file content).
    pub async fn get(&self, id: &str) -> Result<Gist> {
        let url = format!("{API_BASE}/gists/{id}");
        let resp = self.get_with_retry(&url).await?;
        Ok(resp.json().await?)
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
        let url = format!("{API_BASE}/gists");
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
        let url = format!("{API_BASE}/gists/{id}");
        let resp = self
            .request(reqwest::Method::PATCH, &url)
            .json(&body)
            .send()
            .await?;
        check_response(resp).await
    }

    /// Delete a gist.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let url = format!("{API_BASE}/gists/{id}");
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
