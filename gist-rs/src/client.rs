use std::collections::HashMap;

use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, StatusCode};

use crate::error::{GistError, Result};
use crate::types::*;

const API_BASE: &str = "https://api.github.com";

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

    /// List a single page of gists (100 per page). Returns empty vec when no more pages.
    pub async fn list_page(&self, page: u32) -> Result<Vec<Gist>> {
        let url = format!("{API_BASE}/gists?per_page=100&page={page}");
        let resp = self.request(reqwest::Method::GET, &url).send().await?;
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
        let resp = self.request(reqwest::Method::GET, &url).send().await?;
        check_response(resp).await
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
