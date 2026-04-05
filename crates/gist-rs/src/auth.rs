use crate::error::{GistError, Result};

/// Resolve a GitHub token: `$GITHUB_TOKEN` env var, then `gh auth token` fallback.
pub fn resolve_token() -> Result<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }

    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|e| GistError::GhCli(e.to_string()))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    Err(GistError::NoToken)
}
