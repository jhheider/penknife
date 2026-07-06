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

#[cfg(test)]
mod tests {
    use super::*;

    /// The `$GITHUB_TOKEN` fast path returns the env value without shelling out
    /// to `gh`. This is the only test that touches `GITHUB_TOKEN`, so the
    /// process-global mutation does not race another test.
    #[test]
    fn resolve_token_prefers_a_nonempty_env_var() {
        // SAFETY: single-threaded access within this one test; no other test in
        // the crate reads or writes GITHUB_TOKEN.
        let saved = std::env::var("GITHUB_TOKEN").ok();
        unsafe { std::env::set_var("GITHUB_TOKEN", "tok_from_env") };
        assert_eq!(resolve_token().unwrap(), "tok_from_env");

        // An empty env var falls through (does not short-circuit to "").
        unsafe { std::env::set_var("GITHUB_TOKEN", "") };
        // With an empty var and no usable `gh`, this resolves via the gh path
        // or errors; either way it must not return the empty string.
        if let Ok(t) = resolve_token() {
            assert!(!t.is_empty());
        }

        match saved {
            Some(v) => unsafe { std::env::set_var("GITHUB_TOKEN", v) },
            None => unsafe { std::env::remove_var("GITHUB_TOKEN") },
        }
    }
}
