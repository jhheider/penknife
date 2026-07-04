use thiserror::Error;

#[derive(Debug, Error)]
pub enum PkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Gist error: {0}")]
    Gist(#[from] gist_rs::GistError),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PkError>;
