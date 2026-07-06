use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GistError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GitHub API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("no GitHub token found; set $GITHUB_TOKEN or install the `gh` CLI")]
    NoToken,

    #[error("failed to run `gh auth token`: {0}")]
    GhCli(String),

    #[error(
        "gist file {filename} is {size} bytes, beyond the 10MB raw-fetch limit; clone the gist's git repo instead"
    )]
    TooLarge { filename: String, size: u64 },
}

pub type Result<T> = std::result::Result<T, GistError>;
