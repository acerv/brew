// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorStatusLine, EditorTheme, EditorView,
    Highlight, Index2, Lines, RowIndex,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
};

/// Built-in vim-style editor powered by edtui.
pub struct Editor {
    state: EditorState,
    handler: EditorEventHandler,
    title: String,
}

impl Editor {
    /// Create a new editor with the given text content.
    pub fn new(content: &str) -> Self {
        let title = content
            .lines()
            .find_map(|l| l.strip_prefix("Subject:"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Self {
            state: EditorState::new(Lines::from(content)),
            handler: EditorEventHandler::default(),
            title,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Handle a crossterm key event.
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.state.mode = EditorMode::Normal;
            return;
        }
        self.handler.on_key_event(key, &mut self.state);
    }

    /// Return the current editor mode (Normal, Insert, Visual).
    pub fn mode(&self) -> &EditorMode {
        &self.state.mode
    }

    /// Return the full text content as a string.
    pub fn text(&self) -> String {
        self.state.lines.to_string()
    }
}

/// Render the editor into `area`.
pub fn draw(frame: &mut ratatui::Frame, area: Rect, editor: &mut Editor) {
    // Highlight quoted lines ('>' prefix) in blue.
    let quote_style = Style::default().fg(Color::Blue);
    let mut highlights = Vec::new();
    for row in 0..editor.state.lines.len() {
        let Some(line) = editor.state.lines.get(RowIndex::new(row)) else {
            continue;
        };
        let first = line.iter().copied().find(|c| *c != ' ');
        if first == Some('>') && !line.is_empty() {
            highlights.push(Highlight::new(
                Index2::new(row, 0),
                Index2::new(row, line.len().saturating_sub(1)),
                quote_style,
            ));
        }
    }
    editor.state.set_highlights(highlights);

    let mode_style = match editor.state.mode {
        EditorMode::Normal => Style::default().bg(Color::Blue).fg(Color::Black),
        EditorMode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
        EditorMode::Visual => Style::default().bg(Color::Magenta).fg(Color::Black),
        _ => Style::default(),
    };

    let status_line = EditorStatusLine::default()
        .style_mode(mode_style.add_modifier(Modifier::BOLD))
        .style_line(Style::default().fg(Color::DarkGray));

    let theme = EditorTheme::default()
        .base(Style::default().bg(Color::Reset).fg(Color::Reset))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Compose ")
                .title_style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .cursor_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD))
        .selection_style(Style::default().add_modifier(Modifier::REVERSED))
        .line_numbers_style(Style::default().fg(Color::DarkGray))
        .status_line(status_line);

    let view = EditorView::new(&mut editor.state).theme(theme).wrap(true);
    frame.render_widget(view, area);
}
