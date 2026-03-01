// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::cache::{EmailMeta, MailCache};
use anyhow::Result;
use std::path::{Path, PathBuf};

use super::utils::format_timestamp;

/// Format an address list (From / To / Cc) into a single comma-separated string.
/// Returns `"—"` when the header is absent.
pub struct EmailTab {
    pub title: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub date: String,
    pub body: String,
    pub message_id: Option<String>,
    /// Path to the on-disk Maildir file backing this tab.
    pub path: PathBuf,
    pub scroll: u16,
    pub scroll_max: u16,
}

impl EmailTab {
    /// Load the email at `path` and build a tab from it, using `meta` for
    /// subject/timestamp (which may differ from `meta.path` when `mark_seen`
    /// has already renamed the file on disk).
    pub fn from_meta_at(meta: &EmailMeta, path: &Path) -> Result<Self> {
        let msg = MailCache::load_mail(path)?;

        let from = format_addr_list(msg.from());
        let to = format_addr_list(msg.to());
        let cc = format_addr_list(msg.cc());

        let date = format_timestamp(meta.timestamp);

        let body = msg
            .body_text(0)
            .map(|t| t.into_owned())
            .unwrap_or_else(|| "— no text body —".to_string());

        let title = if meta.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            meta.subject.clone()
        };

        let message_id = msg.message_id().map(|s| s.to_owned());

        Ok(Self {
            title,
            from,
            to,
            cc,
            date,
            body,
            message_id,
            path: path.to_path_buf(),
            scroll: 0,
            scroll_max: u16::MAX,
        })
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn format_addr_list(list: Option<&mail_parser::Address>) -> String {
    list.map(|addrs| {
        addrs
            .iter()
            .map(|a| {
                let name = a.name().unwrap_or_default();
                let addr = a.address().unwrap_or_default();
                if name.is_empty() {
                    addr.to_string()
                } else {
                    format!("{} <{}>", name, addr)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_else(|| "—".to_string())
}
