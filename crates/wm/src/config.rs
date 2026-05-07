use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub roots: Vec<PathBuf>,
}

impl Config {
    pub fn data_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("writings-manager")
    }

    fn config_path() -> PathBuf {
        Self::data_dir().join("config.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<Config>(&data) {
                Ok(config) => Ok(config),
                Err(e) => {
                    eprintln!("Warning: invalid config, starting fresh: {e}");
                    Ok(Config { roots: Vec::new() })
                }
            }
        } else {
            Ok(Config { roots: Vec::new() })
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::data_dir();
        std::fs::create_dir_all(&dir)?;
        let data = serde_json::to_string_pretty(self)?;
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
