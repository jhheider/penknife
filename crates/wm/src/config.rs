use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    /// User-defined single-character key → shell-command map. Run from Normal
    /// mode via `sh -c`, with PWD set to the active root. Keys conflicting
    /// with built-in TUI bindings are dropped at load time with a warning.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

impl Config {
    pub fn data_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("writings-manager")
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
                    eprintln!("Warning: invalid config, starting fresh: {e}");
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
        let data = toml::to_string_pretty(self)
            .map_err(|e| crate::error::WmError::Other(format!("toml serialize: {e}")))?;
        std::fs::write(Self::config_path(), data)?;
        Ok(())
    }

    pub fn add_root(&mut self, path: PathBuf) -> Result<()> {
        if !self.roots.contains(&path) {
            self.roots.push(path);
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
}
