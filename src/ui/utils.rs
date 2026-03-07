// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>

/// Fit a string into exactly `width` chars: pad with spaces if shorter, or
/// truncate with `…` if longer.
pub fn fit_string(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= width {
        let mut out = s.to_string();
        out.extend(std::iter::repeat_n(' ', width - char_count));
        out
    } else {
        let mut out: String = s.chars().take(width - 1).collect();
        out.push('…');
        out
    }
}

/// Truncate a string to a certain length, adding '...' char if it overbounds.
pub fn truncate_string(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let mut out = String::with_capacity(max + 1);
    for _ in 0..max {
        match chars.next() {
            Some(c) => out.push(c),
            None => return out,
        }
    }
    if chars.next().is_some() {
        out.pop();
        out.push('…');
    }
    out
}
