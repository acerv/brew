// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Convert the full body string into a `Vec<Line<'static>>` with diff syntax
/// highlighting.
///
/// Tabs are expanded to spaces (8-wide) before styling because ratatui treats
/// tab as a zero-width control character and drops it from the output.
///
/// Call this once when the email is opened and cache the result in
/// [`EmailTab::body_lines`] to avoid re-allocating on every render frame.
pub fn highlight_body(body: &str) -> Vec<Line<'static>> {
    body.lines()
        .map(|raw| {
            let expanded = expand_tabs(raw);
            highlight_line_owned(expanded)
        })
        .collect()
}

/// Expand tab characters to 8-space tab stops.
fn expand_tabs(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0usize;
    for c in s.chars() {
        if c == '\t' {
            let spaces = 8 - (col % 8);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// Takes an owned `String` and returns a syntax-highlighted `Line<'static>`.
fn highlight_line_owned(raw: String) -> Line<'static> {
    let style = if raw.starts_with("---") || raw.starts_with("--- ") {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if raw.starts_with("+++") || raw.starts_with("+++ ") {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if raw.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else if raw.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if raw.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if raw.starts_with('>') {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };
    Line::from(Span::styled(raw, style))
}
