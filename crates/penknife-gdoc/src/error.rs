use thiserror::Error;

#[derive(Debug, Error)]
pub enum GdocError {
    #[error("not authenticated with Google; run the publish flow to sign in")]
    NotAuthenticated,

    #[error("auth error: {0}")]
    Auth(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Drive API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
}

pub type Result<T> = std::result::Result<T, GdocError>;
