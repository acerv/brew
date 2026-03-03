// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use super::app::{App, Entry, SearchField};
use super::tab::EmailTab;
use crate::core::date::humanize_date;
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

/// Groups the mutable list-view state passed from [`App`] into [`draw_list`].
struct ListViewState<'a> {
    selected_mailbox: usize,
    mailbox_list_state: &'a mut ratatui::widgets::ListState,
    thread_list_state: &'a mut ratatui::widgets::ListState,
    labels: &'a [&'a str],
    mailbox_entries: &'a [Vec<Entry>],
    seen_paths: &'a HashMap<String, PathBuf>,
    unread_only: bool,
    search_query: &'a str,
    sender_query: &'a str,
}

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
        let sel = app.selected_mailbox;
        let unread_only = app.unread_only[sel];
        draw_list(
            frame,
            chunks[1],
            ListViewState {
                selected_mailbox: sel,
                mailbox_list_state: &mut app.mailbox_list_state,
                thread_list_state: &mut app.thread_list_states[sel],
                labels,
                mailbox_entries,
                seen_paths: &app.seen_paths,
                unread_only,
                search_query: &app.search_query,
                sender_query: &app.sender_query,
            },
        );
        let entries = &mailbox_entries[sel];
        let selected = app.selected_thread().map(|i| i + 1).unwrap_or(0);
        let filter_hint = if unread_only { "  [unread]" } else { "" };
        let status = if let Some(err) = &app.sync_error {
            Paragraph::new(format!(" sync error: {}", err)).style(Style::default().fg(Color::Red))
        } else if app.search_active {
            let prompt = match app.search_field {
                SearchField::Subject => format!(" /{}_", app.search_query),
                SearchField::Sender => format!(" \\{}_", app.sender_query),
            };
            Paragraph::new(prompt).style(Style::default().fg(Color::Yellow))
        } else {
            let mut hints = String::new();
            if !app.search_query.is_empty() {
                hints += &format!("  [/{}]", app.search_query);
            }
            if !app.sender_query.is_empty() {
                hints += &format!("  [\\{}]", app.sender_query);
            }
            Paragraph::new(format!(
                " {}/{}{}{} — j/k move  J/K mailbox  Enter open  r reply  R reply-empty  N show-unread  n show-all  / search  \\ sender  h/l tabs  Q quit",
                selected,
                entries.len(),
                filter_hint,
                hints,
            ))
            .style(Style::default().fg(Color::DarkGray))
        };
        frame.render_widget(status, chunks[2]);
    } else {
        let ei = app.active - 1;
        draw_email(frame, &mut app.emails[ei], chunks[1]);
        let status = if let Some(err) = &app.sync_error {
            Paragraph::new(format!(" sync error: {}", err)).style(Style::default().fg(Color::Red))
        } else {
            Paragraph::new(
                " j/k scroll  J/K email  h/l tabs  r reply  R reply-empty  Esc back  q close",
            )
            .style(Style::default().fg(Color::DarkGray))
        };
        frame.render_widget(status, chunks[2]);
    }
}

fn draw_list(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: ListViewState<'_>) {
    let ListViewState {
        selected_mailbox,
        mailbox_list_state,
        thread_list_state,
        labels,
        mailbox_entries,
        seen_paths,
        unread_only,
        search_query,
        sender_query,
    } = state;
    let left_w = (labels.iter().map(|l| l.len()).max().unwrap_or(8) + 18).clamp(16, 40) as u16;
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
    let filter_marker = if unread_only {
        " Mailbox [unread] "
    } else {
        " Mailbox "
    };
    let mb_list = List::new(mb_items)
        .block(Block::default().borders(Borders::ALL).title(filter_marker))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(mb_list, panes[0], mailbox_list_state);

    // ── right: thread list for the selected mailbox ──
    // Layout per row: [from: FROM_W] [indent+subject: subject_w] [date: DATE_W]
    // The three columns plus two single-space separators fill the usable width.
    const FROM_W: usize = 27;
    const DATE_W: usize = 16; // "YYYY-MM-DD HH:MM"
    let usable = panes[1].width.saturating_sub(2) as usize; // subtract borders
    let subject_w = usable.saturating_sub(FROM_W + DATE_W + 2);

    let entries = &mailbox_entries[selected_mailbox];
    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| {
            let from = fit(&e.thread.data.from, FROM_W);
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
            let subject_avail = subject_w.saturating_sub(indent.chars().count());
            let subject_padded = fit(&subject, subject_avail);
            let eff_path = seen_paths
                .get(&e.thread.data.message_id)
                .map(|p| p.as_path())
                .unwrap_or(&e.thread.data.path);
            let text_style = if is_unread(eff_path) {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(from, text_style),
                Span::raw(" "),
                Span::styled(indent, Style::default().fg(Color::DarkGray)),
                Span::styled(subject_padded, text_style),
                Span::raw(" "),
                Span::styled(
                    format!("{:<DATE_W$}", humanize_date(e.thread.data.timestamp)),
                    Style::default().fg(Color::Cyan),
                ),
            ]))
        })
        .collect();
    let threads_title = match (!search_query.is_empty(), !sender_query.is_empty()) {
        (true, true) => format!(" Threads [/{} \\{}] ", search_query, sender_query),
        (true, false) => format!(" Threads [/{}] ", search_query),
        (false, true) => format!(" Threads [\\{}] ", sender_query),
        (false, false) => " Threads ".to_string(),
    };
    let thread_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(threads_title))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(thread_list, panes[1], thread_list_state);
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

    let body_lines = &tab.body_lines;

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

    let body = Paragraph::new(body_lines.to_vec())
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((tab.scroll, 0));
    frame.render_widget(body, chunks[1]);
}

// ── helpers ─────────────────────────────────────────────────────────────────

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

/// Fit `s` into exactly `width` terminal columns: truncate (with `…`) if too
/// long, right-pad with spaces if too short.
fn fit(s: &str, width: usize) -> String {
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
