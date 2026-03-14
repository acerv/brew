// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use super::app::{App, MoveMode, SearchMode, Tab};
use crate::core::config;
use crate::core::maildir::{Maildir, SortOrder};
use crate::ui::utils;
use crate::ui::{editor, email, threads};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, LineGauge, List, ListItem, ListState, Paragraph, Tabs};

pub fn draw_startup(frame: &mut ratatui::Frame, label: &str, current: usize, total: usize) {
    let area = frame.area();
    let w = 50u16.min(area.width);
    let x = area.width.saturating_sub(w) / 2;
    let y = area.height / 2;

    let ratio = if total == 0 {
        1.0
    } else {
        (current + 1) as f64 / total as f64
    };
    let pct = (ratio * 100.0) as u16;

    frame.render_widget(
        Paragraph::new(format!(" {label}")).style(Style::default().fg(Color::DarkGray)),
        ratatui::layout::Rect::new(x, y, w, 1),
    );
    frame.render_widget(
        LineGauge::default()
            .filled_style(Style::default().fg(Color::Cyan))
            .unfilled_style(Style::default().fg(Color::DarkGray))
            .ratio(ratio)
            .label(format!("{pct}%")),
        ratatui::layout::Rect::new(x, y + 1, w, 1),
    );
}

pub fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let titles: Vec<String> = std::iter::once("Brew".to_string())
        .chain(app.tabs.iter().map(|tab| match tab {
            Tab::Email(ev) => utils::truncate_string(ev.subject(), 20),
            Tab::Compose(ed, _) => {
                let t = ed.title();
                if t.is_empty() {
                    "Compose".to_string()
                } else {
                    utils::truncate_string(t, 20)
                }
            }
        }))
        .map(|s| format!(" {s} "))
        .collect();

    let tab_bar = Tabs::new(titles)
        .select(app.current_tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tab_bar, chunks[0]);
    frame.render_widget(Block::default().borders(Borders::TOP), chunks[1]);

    if app.current_tab == 0 {
        draw_main(frame, chunks[2], app);
    } else if let Some(tab) = app.tabs.get_mut(app.current_tab.saturating_sub(1)) {
        match tab {
            Tab::Email(ev) => email::draw(frame, chunks[2], ev),
            Tab::Compose(ed, _) => editor::draw(frame, chunks[2], ed),
        }
    }

    draw_statusbar(frame, chunks[3], app);
    draw_move_popup(frame, app);
}

pub fn draw_main(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(0)])
        .split(area);

    app.sidebar_state.select(Some(app.current_mb));
    let unread_filters: Vec<bool> = app.threads.iter().map(|tv| tv.is_unread_only()).collect();
    draw_sidebar(
        frame,
        chunks[0],
        &mut app.sidebar_state,
        &app.config.mailboxes,
        &app.maildirs,
        &unread_filters,
    );

    if let Some(tv) = app.threads.get_mut(app.current_mb) {
        threads::draw(frame, chunks[1], tv);
    }
}

pub fn draw_sidebar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &mut ListState,
    mailboxes: &[config::Mailbox],
    maildirs: &[Maildir],
    unread_filters: &[bool],
) {
    let items: Vec<ListItem> = mailboxes
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let unread = maildirs
                .iter()
                .find(|md| md.path() == m.path)
                .map(|md| md.unread_count())
                .unwrap_or(0);
            let filter_active = unread_filters.get(i).copied().unwrap_or(false);
            let (text, style) = if unread > 0 {
                (
                    format!("{} ({})", m.label, unread),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (m.label.clone(), Style::default())
            };
            let mut spans = vec![Span::styled(text, style)];
            if filter_active {
                spans.push(Span::styled(" [u]", Style::default().fg(Color::Yellow)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let widget = List::new(items)
        .block(Block::default().borders(Borders::RIGHT))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(widget, area, state);
}

fn draw_statusbar(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let widget = if let SearchMode::Typing(input) = &app.search {
        Paragraph::new(format!(" /{input}_")).style(Style::default().fg(Color::Yellow))
    } else if let Some(err) = &app.status_error {
        Paragraph::new(format!(" error: {err}")).style(Style::default().fg(Color::Red))
    } else if app.current_tab == 0 {
        let mut spans = vec![Span::styled(
            " j/k↑↓ move  J/K mailbox  r reply  R reply+quote  C compose  / search  Q quit",
            Style::default().fg(Color::DarkGray),
        )];
        if let Some(md) = app.maildirs.get(app.current_mb)
            && md.sort_order() == SortOrder::Ascending
        {
            spans.push(Span::styled("  |  ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                "sort: asc",
                Style::default().fg(Color::Yellow),
            ));
        }
        if let Some(tv) = app.threads.get(app.current_mb)
            && let Some(q) = tv.search()
        {
            spans.push(Span::styled("  |  ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!("search: {q}"),
                Style::default().fg(Color::Yellow),
            ));
        }
        Paragraph::new(Line::from(spans))
    } else {
        Paragraph::new(" j/k scroll  J/K email  r reply  R reply+quote  C compose  q close")
            .style(Style::default().fg(Color::DarkGray))
    };
    frame.render_widget(widget, area);
}

fn draw_move_popup(frame: &mut ratatui::Frame, app: &App) {
    let MoveMode::Active { selected } = app.move_mode else {
        return;
    };

    let labels: Vec<String> = app
        .config
        .mailboxes
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != app.current_mb)
        .map(|(_, mb)| mb.label.clone())
        .collect();

    if !labels.is_empty() {
        draw_list_popup(frame, " Move to ", &labels, selected);
    }
}

pub fn draw_list_popup(frame: &mut ratatui::Frame, title: &str, items: &[String], selected: usize) {
    use ratatui::widgets::Clear;

    let max_label = items.iter().map(|l| l.len()).max().unwrap_or(10);
    let popup_w = (max_label as u16 + 8).clamp(22, 40);
    let popup_h = items.len() as u16 + 4;

    let area = frame.area();
    let x = area.width.saturating_sub(popup_w) / 2;
    let y = area.height.saturating_sub(popup_h) / 2;
    let popup_area =
        ratatui::layout::Rect::new(x, y, popup_w.min(area.width), popup_h.min(area.height));

    frame.render_widget(Clear, popup_area);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        title.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let list_items: Vec<ListItem> = items
        .iter()
        .map(|l| ListItem::new(format!(" {l} ")))
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected));

    let list = List::new(list_items)
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, inner_chunks[1], &mut state);
}
