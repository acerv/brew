use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Mailbox {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(rename = "mailbox")]
    pub mailboxes: Vec<Mailbox>,
}

impl Config {
    /// Load and parse the config file.
    ///
    /// Looks for the file at `$XDG_CONFIG_HOME/brew/config.toml`,
    /// falling back to `~/.config/brew/config.toml`.
    pub fn load() -> Result<Self> {
        let path = config_path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read config file: {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("cannot parse config file: {}", path.display()))
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".config"));
    base.join("brew").join("config.toml")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
