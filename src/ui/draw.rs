// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use super::app::{App, Entry};
use super::tab::EmailTab;
use super::utils::format_timestamp;
use crate::core::read::is_unread;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap},
};
use std::collections::HashMap;
use std::path::PathBuf;

// ── draw functions ────────────────────────────────────────────────────────────

pub fn draw(
    frame: &mut ratatui::Frame,
    app: &mut App,
    labels: &[&str],
    mailbox_entries: &[Vec<Entry>],
) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    // ── tab bar ──
    let mut titles: Vec<Line> = vec![Line::from(Span::raw("Brew"))];
    for e in &app.emails {
        titles.push(Line::from(truncate(&e.title, 20)));
    }
    let tab_bar = Tabs::new(titles)
        .select(app.active)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
    frame.render_widget(tab_bar, chunks[0]);

    // ── content + status bar ──
    if app.active == 0 {
        let seen = app.seen_paths.clone();
        draw_list(frame, app, labels, mailbox_entries, chunks[1], &seen);
        let entries = &mailbox_entries[app.selected_mailbox];
        let selected = app.selected_thread().map(|i| i + 1).unwrap_or(0);
        let status = Paragraph::new(format!(
            " {}/{} — j/k move  J/K mailbox  Enter open  r reply  R reply-empty  t thanks  h/l tabs  q quit",
            selected,
            entries.len(),
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    } else {
        let ei = app.active - 1;
        draw_email(frame, &mut app.emails[ei], chunks[1]);
        let status = Paragraph::new(
            " j/k scroll  J/K thread  h/l tabs  r reply  R reply-empty  t thanks  Esc back  q close",
        )
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    }
}

fn draw_list(
    frame: &mut ratatui::Frame,
    app: &mut App,
    labels: &[&str],
    mailbox_entries: &[Vec<Entry>],
    area: ratatui::layout::Rect,
    seen_paths: &HashMap<String, PathBuf>,
) {
    let left_w = (labels.iter().map(|l| l.len()).max().unwrap_or(8) + 10).clamp(16, 40) as u16;
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_w), Constraint::Min(0)])
        .split(area);

    // ── left: mailbox list ──
    let mb_items: Vec<ListItem> = labels
        .iter()
        .zip(mailbox_entries.iter())
        .map(|(label, entries)| {
            let unread: usize = entries
                .iter()
                .filter(|e| {
                    let eff = seen_paths
                        .get(&e.thread.data.message_id)
                        .map(|p| p.as_path())
                        .unwrap_or(&e.thread.data.path);
                    is_unread(eff)
                })
                .count();
            let (text, style) = if unread > 0 {
                (
                    format!("{} ({})", label, unread),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (label.to_string(), Style::default())
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();
    let mb_list = List::new(mb_items)
        .block(Block::default().borders(Borders::ALL).title(" Mailbox "))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(mb_list, panes[0], &mut app.mailbox_list_state);

    // ── right: thread list for the selected mailbox ──
    let entries = &mailbox_entries[app.selected_mailbox];
    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| {
            let date = format_timestamp(e.thread.data.timestamp);
            let indent = if e.depth == 0 {
                String::new()
            } else {
                format!("{}└ ", "  ".repeat(e.depth - 1))
            };
            let subject = if e.thread.data.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                e.thread.data.subject.clone()
            };
            let eff_path = seen_paths
                .get(&e.thread.data.message_id)
                .map(|p| p.as_path())
                .unwrap_or(&e.thread.data.path);
            let subject_style = if is_unread(eff_path) {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(date, Style::default().fg(Color::Cyan)),
                Span::styled(indent, Style::default().fg(Color::DarkGray)),
                Span::styled(subject, subject_style),
            ]))
        })
        .collect();
    let thread_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Threads "))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(
        thread_list,
        panes[1],
        &mut app.thread_list_states[app.selected_mailbox],
    );
}

fn draw_email(frame: &mut ratatui::Frame, tab: &mut EmailTab, area: ratatui::layout::Rect) {
    const LABEL: usize = 7; // "From : " / "To   : " / "Cc   : " / "Date : "
    let inner_width = area.width.saturating_sub(2) as usize;
    let value_width = inner_width.saturating_sub(LABEL).max(1);

    let from_lines = wrap_header_field("From : ", &tab.from, value_width);
    let to_lines = wrap_header_field("To   : ", &tab.to, value_width);
    let cc_lines = wrap_header_field("Cc   : ", &tab.cc, value_width);

    let header_height = (from_lines.len() + to_lines.len() + cc_lines.len() + 1 + 2) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(0)])
        .split(area);

    let mut header_text: Vec<Line> = Vec::new();
    header_text.extend(from_lines);
    header_text.extend(to_lines);
    header_text.extend(cc_lines);
    header_text.push(Line::from(vec![
        Span::styled("Date : ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(tab.date.trim().to_string()),
    ]));

    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::ALL).title(
        Span::styled(
            format!(" {} ", truncate(&tab.title, 60)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ));
    frame.render_widget(header, chunks[0]);

    let body_lines = highlight_body(&tab.body);

    let inner_width = chunks[1].width.saturating_sub(2) as usize;
    let visible_height = chunks[1].height.saturating_sub(2) as usize;
    let total_lines: usize = if inner_width == 0 {
        body_lines.len()
    } else {
        body_lines
            .iter()
            .map(|l| {
                let char_len: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
                if char_len == 0 {
                    1
                } else {
                    char_len.div_ceil(inner_width)
                }
            })
            .sum()
    };
    tab.scroll_max = total_lines.saturating_sub(visible_height) as u16;
    tab.scroll = tab.scroll.min(tab.scroll_max);

    let body = Paragraph::new(body_lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((tab.scroll, 0));
    frame.render_widget(body, chunks[1]);
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Convert the full body string into a `Vec<Line>` with diff syntax highlighting.
///
/// Tabs are expanded to spaces (8-wide) before styling because ratatui treats
/// tab as a zero-width control character and drops it from the output.
fn highlight_body(body: &str) -> Vec<Line<'static>> {
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

/// Wrap a header field value into one or more `Line`s.
///
/// The label (e.g. `"From : "`) appears on the first line, and continuation
/// lines are indented by the same width so values stay visually aligned.
fn wrap_header_field<'a>(label: &'a str, value: &'a str, value_width: usize) -> Vec<Line<'a>> {
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut chars = value.chars();
    let mut lines: Vec<Line<'a>> = Vec::new();

    let first: String = chars.by_ref().take(value_width).collect();
    lines.push(Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(first),
    ]));

    let indent = " ".repeat(label.chars().count());
    loop {
        let chunk: String = chars.by_ref().take(value_width).collect();
        if chunk.is_empty() {
            break;
        }
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::raw(chunk),
        ]));
    }

    lines
}

fn truncate(s: &str, max: usize) -> String {
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

