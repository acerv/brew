// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Mailbox {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub name: Option<String>,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct Sync {
    pub command: String,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(rename = "mailbox")]
    pub mailboxes: Vec<Mailbox>,
    pub smtp: Smtp,
    pub sync: Option<Sync>,
}

impl Config {
    /// Load and parse the config file.
    ///
    /// Looks for the file at `$XDG_CONFIG_HOME/brew/config.toml`,
    /// falling back to `~/.config/brew/config.toml`.
    pub fn load() -> Result<Self> {
        let path = config_dir().join("config.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read config file: {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("cannot parse config file: {}", path.display()))
    }
}

/// Read `signature` from the brew config directory if it exists.
pub fn load_signature() -> Option<String> {
    std::fs::read_to_string(config_dir().join("signature")).ok()
}

/// Read `thanks` from the brew config directory if it exists.
pub fn load_thanks() -> Option<String> {
    std::fs::read_to_string(config_dir().join("thanks")).ok()
}

/// The brew configuration directory (`$XDG_CONFIG_HOME/brew` or
/// `~/.config/brew`).
pub fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".config"));
    base.join("brew")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
