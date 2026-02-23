use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gist {
    pub id: String,
    pub html_url: String,
    pub description: Option<String>,
    pub public: bool,
    pub files: HashMap<String, GistFile>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GistFile {
    pub filename: String,
    pub size: u64,
    #[serde(default)]
    pub raw_url: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateGistRequest {
    pub description: String,
    pub public: bool,
    pub files: HashMap<String, GistFileContent>,
}

#[derive(Debug, Serialize)]
pub struct UpdateGistRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub files: HashMap<String, GistFileContent>,
}

#[derive(Debug, Serialize)]
pub struct GistFileContent {
    pub content: String,
}
