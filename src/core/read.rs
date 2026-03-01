// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
//! Maildir read-status detection.
//!
//! Maildir encodes message flags in the filename itself:
//!
//! - Files under `new/` have never been delivered to a mail client — **unread**.
//! - Files under `cur/` carry flags after `:2,`.  The `S` flag means Seen.
//!   - `:2,S` or `:2,FS` etc. → **read**
//!   - `:2,`  (no S)         → **unread**
//!   - No `:2,` info at all  → treated as **unread** (conservative default)
//!
//! mbsync writes and syncs these flags with the IMAP server, so this gives
//! an accurate, server-consistent view without any extra state files.

use std::path::{Path, PathBuf};

/// Returns `true` when the Maildir file at `path` should be considered unread.
pub fn is_unread(path: &Path) -> bool {
    // Files sitting in a `new/` directory are always unread.
    if path.components().any(|c| c.as_os_str() == "new") {
        return true;
    }

    // For files in `cur/`, parse the info field from the filename.
    // Maildir info starts after the last `:` in the filename.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Find the `:2,` flags section.
    if let Some(info_start) = name.rfind(':') {
        let info = &name[info_start + 1..];
        // Standard info field starts with "2,"; if not present treat as unread.
        if let Some(flags) = info.strip_prefix("2,") {
            return !flags.contains('S');
        }
    }

    // No flags info — conservative: treat as unread.
    true
}

/// Rename the Maildir file at `path` to mark it as Seen (`S` flag).
///
/// - Files in `new/` are moved to `cur/` with `:2,S` appended.
/// - Files in `cur/` have `S` inserted into their flags if not already present.
/// - Already-seen files are left untouched.
///
/// Errors are silently ignored — a failed rename just means the status
/// won't be synced back to the server, which is not fatal.
/// Returns the new path after renaming (same as `path` if already seen or rename failed).
pub fn mark_seen(path: &Path) -> PathBuf {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_owned(),
        None => return path.to_path_buf(),
    };
    let dir = match path.parent() {
        Some(d) => d,
        None => return path.to_path_buf(),
    };

    let new_name = if path.components().any(|c| c.as_os_str() == "new") {
        // Move from new/ to cur/ and add :2,S
        format!("{}:2,S", name)
    } else {
        // Already in cur/ — patch the flags field
        if let Some(colon) = name.rfind(':') {
            let base = &name[..colon];
            let info = &name[colon + 1..];
            if let Some(flags) = info.strip_prefix("2,") {
                if flags.contains('S') {
                    return path.to_path_buf(); // already seen
                }
                // Insert S in alphabetical order among existing flags
                let mut chars: Vec<char> = flags.chars().collect();
                chars.push('S');
                chars.sort_unstable();
                let new_flags: String = chars.into_iter().collect();
                format!("{}:2,{}", base, new_flags)
            } else {
                // Unrecognised info format — append :2,S fresh
                format!("{}:2,S", name)
            }
        } else {
            // No flags at all
            format!("{}:2,S", name)
        }
    };

    if new_name == name {
        return path.to_path_buf();
    }

    // Build destination path: always in cur/
    let cur_dir = if path.components().any(|c| c.as_os_str() == "new") {
        dir.parent().unwrap_or(dir).join("cur")
    } else {
        dir.to_path_buf()
    };

    let dest = cur_dir.join(&new_name);
    if std::fs::rename(path, &dest).is_ok() {
        dest
    } else {
        path.to_path_buf()
    }
}
