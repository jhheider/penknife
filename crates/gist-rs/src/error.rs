use thiserror::Error;

#[derive(Debug, Error)]
pub enum GistError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GitHub API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("No GitHub token found. Set $GITHUB_TOKEN or install `gh` CLI.")]
    NoToken,

    #[error("Failed to run `gh auth token`: {0}")]
    GhCli(String),

    #[error(
        "Gist file {filename} is {size} bytes — beyond the 10MB raw-fetch limit; clone the gist's git repo instead"
    )]
    TooLarge { filename: String, size: u64 },
}

pub type Result<T> = std::result::Result<T, GistError>;
