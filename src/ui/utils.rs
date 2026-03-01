// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use time::OffsetDateTime;
use time::macros::format_description;

/// Format a Unix timestamp into a fixed-width "YYYY-MM-DD HH:MM" string.
pub fn format_timestamp(ts: Option<i64>) -> String {
    const DATE_WIDTH: usize = 17; // "YYYY-MM-DD HH:MM" + 1 space

    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]");
    match ts.and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok()) {
        Some(dt) => format!("{:<DATE_WIDTH$}", dt.format(fmt).unwrap_or_default()),
        None => format!("{:<DATE_WIDTH$}", "—"),
    }
}
