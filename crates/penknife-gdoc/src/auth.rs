//! OAuth 2.0 device flow against Google, with an on-disk token cache.
//!
//! The device flow is the right shape for a terminal app: we show the user a
//! short code and a URL, they approve in any browser (even on another
//! machine), and we poll for the token. Only the non-sensitive `drive.file`
//! scope is requested, so the consent screen needs no Google verification
//! review. The "client secret" of an installed app is not confidential by
//! Google's own doctrine; embedding one in an open-source binary is the
//! accepted pattern (rclone does the same).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{GdocError, Result};

pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const DEFAULT_AUTH_BASE: &str = "https://oauth2.googleapis.com";

/// OAuth client credentials. Resolved by the app from config, environment,
/// or a compile-time default; never persisted by this crate.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

/// A pending device authorization: show `user_code` and `verification_url`
/// to the user, then poll with [`Authenticator::poll_device_flow`].
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuth {
    pub device_code: String,
    pub user_code: String,
    /// Google sends `verification_url`; RFC 8628 calls it `verification_uri`.
    #[serde(alias = "verification_uri")]
    pub verification_url: String,
    /// Seconds between polls, per the server.
    #[serde(default = "default_interval")]
    pub interval: u64,
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

/// Cached tokens, persisted as JSON with owner-only permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl TokenSet {
    fn is_fresh(&self) -> bool {
        // A minute of slack so a token never expires mid-request.
        self.expires_at > Utc::now() + chrono::Duration::seconds(60)
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
}

pub struct Authenticator {
    creds: Credentials,
    cache_path: PathBuf,
    auth_base: String,
    http: reqwest::Client,
}

impl Authenticator {
    pub fn new(creds: Credentials, cache_path: PathBuf) -> Self {
        Self::with_auth_base(creds, cache_path, DEFAULT_AUTH_BASE.to_string())
    }

    /// Construct against a non-default OAuth base URL (mock servers in tests).
    pub fn with_auth_base(creds: Credentials, cache_path: PathBuf, auth_base: String) -> Self {
        Self {
            creds,
            cache_path,
            auth_base,
            http: reqwest::Client::new(),
        }
    }

    /// A valid access token, refreshing (and re-persisting) if the cached
    /// one has expired. `Err(NotAuthenticated)` means the caller must run
    /// the device flow.
    pub async fn access_token(&self) -> Result<String> {
        let Some(cached) = self.load_cache() else {
            return Err(GdocError::NotAuthenticated);
        };
        if cached.is_fresh() {
            return Ok(cached.access_token);
        }
        let Some(refresh) = cached.refresh_token.clone() else {
            return Err(GdocError::NotAuthenticated);
        };
        let refreshed = self.refresh(&refresh).await?;
        Ok(refreshed.access_token)
    }

    /// Whether a token cache exists at all (fresh or not). Lets the UI know
    /// if publish will need the device-flow dialog first.
    pub fn has_cached_token(&self) -> bool {
        self.load_cache().is_some()
    }

    /// Begin the device flow: returns the code and URL to show the user.
    pub async fn start_device_flow(&self) -> Result<DeviceAuth> {
        let resp = self
            .http
            .post(format!("{}/device/code", self.auth_base))
            .form(&[
                ("client_id", self.creds.client_id.as_str()),
                ("scope", DRIVE_FILE_SCOPE),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GdocError::Auth(format!(
                "device code request failed (HTTP {}): {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(resp.json().await?)
    }

    /// Poll until the user approves (or the flow fails). Respects the
    /// server's requested interval, including `slow_down` responses.
    pub async fn poll_device_flow(&self, auth: &DeviceAuth) -> Result<TokenSet> {
        let deadline = Utc::now() + chrono::Duration::seconds(auth.expires_in as i64);
        let mut interval = auth.interval.max(1);
        loop {
            if Utc::now() > deadline {
                return Err(GdocError::Auth("device code expired".into()));
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

            let resp = self
                .http
                .post(format!("{}/token", self.auth_base))
                .form(&[
                    ("client_id", self.creds.client_id.as_str()),
                    ("client_secret", self.creds.client_secret.as_str()),
                    ("device_code", auth.device_code.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?;

            if resp.status().is_success() {
                let token: TokenResponse = resp.json().await?;
                let set = TokenSet {
                    access_token: token.access_token,
                    refresh_token: token.refresh_token,
                    expires_at: Utc::now() + chrono::Duration::seconds(token.expires_in),
                };
                self.save_cache(&set)?;
                return Ok(set);
            }

            let err: TokenErrorResponse = resp.json().await.unwrap_or(TokenErrorResponse {
                error: "unknown".into(),
            });
            match err.error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => interval += 5,
                other => {
                    return Err(GdocError::Auth(format!("device flow failed: {other}")));
                }
            }
        }
    }

    /// Exchange a refresh token for a fresh access token and persist it.
    /// Google may omit the refresh token in the response; keep the old one.
    async fn refresh(&self, refresh_token: &str) -> Result<TokenSet> {
        let resp = self
            .http
            .post(format!("{}/token", self.auth_base))
            .form(&[
                ("client_id", self.creds.client_id.as_str()),
                ("client_secret", self.creds.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            // A revoked refresh token means re-auth, not a hard error.
            return Err(GdocError::NotAuthenticated);
        }
        let token: TokenResponse = resp.json().await?;
        let set = TokenSet {
            access_token: token.access_token,
            refresh_token: token
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            expires_at: Utc::now() + chrono::Duration::seconds(token.expires_in),
        };
        self.save_cache(&set)?;
        Ok(set)
    }

    fn load_cache(&self) -> Option<TokenSet> {
        let data = std::fs::read_to_string(&self.cache_path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Atomic write (temp + rename), owner-only on unix: these are tokens.
    fn save_cache(&self, set: &TokenSet) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.cache_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(set)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.cache_path)?;
        Ok(())
    }
}
