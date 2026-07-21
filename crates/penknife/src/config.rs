use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use color_eyre::eyre::{Result, WrapErr};

/// A watched root directory, with optional per-root ignore patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    /// Glob patterns (gitignore-style, matched against rel_path) of files to
    /// skip in the tree. Recursive patterns require `**`; `*.md` matches a
    /// single path segment, not subdirectories.
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl Root {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            ignore: Vec::new(),
        }
    }
}

/// How files in the tree are ordered. Defaults to modification-time descending.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    MtimeDesc,
    MtimeAsc,
    AlphaAsc,
    AlphaDesc,
    /// Sync-state grouped: Conflict → LocalNewer → RemoteNewer → NotGisted →
    /// Synced, with mtime-desc within each bucket.
    Status,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::MtimeDesc => "newest first",
            Self::MtimeAsc => "oldest first",
            Self::AlphaAsc => "A → Z",
            Self::AlphaDesc => "Z → A",
            Self::Status => "by status",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::MtimeDesc => "mtime↓",
            Self::MtimeAsc => "mtime↑",
            Self::AlphaAsc => "alpha",
            Self::AlphaDesc => "alpha↓",
            Self::Status => "status",
        }
    }

    pub fn all() -> &'static [SortMode] {
        &[
            Self::MtimeDesc,
            Self::MtimeAsc,
            Self::AlphaAsc,
            Self::AlphaDesc,
            Self::Status,
        ]
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SortConfig {
    #[serde(default)]
    pub mode: SortMode,
}

/// Background polling cadence, in seconds. Zero disables that poller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollConfig {
    /// Interval between background remote checks (one cheap API listing per
    /// check; failures back off exponentially on top of this).
    #[serde(default = "default_remote_secs")]
    pub remote_secs: u64,
    /// Interval between local filesystem sweeps that pick up files created,
    /// edited, or deleted outside the TUI.
    #[serde(default = "default_local_secs")]
    pub local_secs: u64,
}

fn default_remote_secs() -> u64 {
    300
}

fn default_local_secs() -> u64 {
    5
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            remote_secs: default_remote_secs(),
            local_secs: default_local_secs(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub roots: Vec<Root>,
    #[serde(default)]
    pub sort: SortConfig,
    #[serde(default)]
    pub poll: PollConfig,
    /// User-defined single-character key → shell-command map. Run from Normal
    /// mode via `sh -c`, with PWD set to the active root. Keys conflicting
    /// with built-in TUI bindings are dropped at load time with a warning.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

impl Config {
    pub fn data_dir() -> PathBuf {
        let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("penknife");
        // One-shot migration from the pre-rename data dir. Only fires while the
        // new dir doesn't exist; a failed rename falls back to the old dir so
        // state is never split across both.
        if !dir.exists() {
            let legacy = base.join("writings-manager");
            if legacy.exists() && std::fs::rename(&legacy, &dir).is_err() {
                return legacy;
            }
        }
        dir
    }

    pub fn config_path() -> PathBuf {
        Self::data_dir().join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            match toml::from_str::<Config>(&data) {
                Ok(config) => Ok(config),
                Err(e) => {
                    eprintln!("penknife: warning: invalid config, starting fresh: {e}");
                    Ok(Config::default())
                }
            }
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::data_dir();
        std::fs::create_dir_all(&dir)?;
        let data = toml::to_string_pretty(self).wrap_err("toml serialize")?;
        std::fs::write(Self::config_path(), data)?;
        Ok(())
    }

    pub fn add_root(&mut self, path: PathBuf) -> Result<()> {
        if !self.roots.iter().any(|r| r.path == path) {
            self.roots.push(Root::new(path));
            self.save()?;
        }
        Ok(())
    }

    pub fn remove_root(&mut self, index: usize) -> Result<()> {
        if index < self.roots.len() {
            self.roots.remove(index);
            self.save()?;
        }
        Ok(())
    }

    /// Convenience accessor for ignore globs of the root at `path`. Returns
    /// an empty slice if the path isn't a configured root.
    #[allow(dead_code)] // wired up in task #54
    pub fn ignore_for(&self, path: &Path) -> &[String] {
        self.roots
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.ignore.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_defaults_are_enabled() {
        let p = PollConfig::default();
        assert_eq!(p.remote_secs, 300);
        assert_eq!(p.local_secs, 5);
    }

    #[test]
    fn poll_config_fills_missing_fields() {
        // A config written before polling existed (or with a partial [poll]
        // table) must deserialize with defaults, not error.
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.poll.remote_secs, 300);
        let cfg: Config = toml::from_str("[poll]\nremote_secs = 60\n").unwrap();
        assert_eq!(cfg.poll.remote_secs, 60);
        assert_eq!(cfg.poll.local_secs, 5);
    }

    #[test]
    fn poll_zero_means_disabled_roundtrips() {
        let cfg: Config = toml::from_str("[poll]\nremote_secs = 0\nlocal_secs = 0\n").unwrap();
        assert_eq!(cfg.poll.remote_secs, 0);
        assert_eq!(cfg.poll.local_secs, 0);
    }
}
