// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::address::AddressBook;
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorStatusLine, EditorTheme, EditorView,
    Highlight, Index2, Lines, RowIndex,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

const BODY_SENTINEL: &str = "--- body ---";

/// Which part of the compose view is focused.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    To,
    Cc,
    Subject,
    Body,
}

/// A simple single-line text input with cursor.
struct HeaderField {
    label: &'static str,
    value: String,
    cursor: usize,
}

impl HeaderField {
    fn new(label: &'static str, value: &str) -> Self {
        let cursor = value.len();
        Self {
            label,
            value: value.to_string(),
            cursor,
        }
    }

    fn on_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.value[..self.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.value.remove(prev);
                    self.cursor = prev;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = self.value[..self.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.cursor < self.value.len() {
                    self.cursor += self.value[self.cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            _ => {}
        }
    }

    /// The query fragment for autocomplete: text after the last comma.
    fn current_fragment(&self) -> &str {
        self.value
            .rfind(',')
            .map(|i| self.value[i + 1..].trim_start())
            .unwrap_or(self.value.trim())
    }

    /// Replace the fragment being typed with `addr` and append `, ` for the next entry.
    fn accept_suggestion(&mut self, addr: &str) {
        if let Some(comma_pos) = self.value.rfind(',') {
            self.value.truncate(comma_pos + 1);
            self.value.push(' ');
        } else {
            self.value.clear();
        }
        self.value.push_str(addr);
        self.value.push_str(", ");
        self.cursor = self.value.len();
    }
}

/// Built-in compose view with header fields and vim-style body editor.
pub struct Editor {
    to: HeaderField,
    cc: HeaderField,
    subject: HeaderField,
    in_reply_to: Option<String>,
    body_state: EditorState,
    body_handler: EditorEventHandler,
    focus: Focus,
    suggestions: Vec<String>,
    ac_selected: usize,
}

impl Editor {
    /// Create a new compose editor by parsing a draft string.
    pub fn new(content: &str) -> Self {
        let mut to = String::new();
        let mut cc = String::new();
        let mut subject = String::new();
        let mut in_reply_to = None;

        for line in content.lines().take_while(|l| *l != BODY_SENTINEL) {
            if let Some(val) = line.strip_prefix("To:") {
                to = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("Cc:") {
                cc = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("Subject:") {
                subject = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("In-Reply-To:") {
                in_reply_to = Some(val.trim().to_string());
            }
        }

        let body = content
            .lines()
            .position(|l| l == BODY_SENTINEL)
            .map(|i| content.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();

        Self {
            to: HeaderField::new("To", &to),
            cc: HeaderField::new("Cc", &cc),
            subject: HeaderField::new("Subject", &subject),
            in_reply_to,
            body_state: EditorState::new(Lines::from(body.as_str())),
            body_handler: EditorEventHandler::default(),
            focus: Focus::To,
            suggestions: Vec::new(),
            ac_selected: 0,
        }
    }

    pub fn title(&self) -> &str {
        &self.subject.value
    }

    /// Handle a crossterm key event.
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Ctrl+C → Normal mode (body) or do nothing (headers).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.focus == Focus::Body {
                self.body_state.mode = EditorMode::Normal;
            }
            self.suggestions.clear();
            return;
        }

        // Autocomplete navigation when suggestions are showing.
        if !self.suggestions.is_empty() {
            match key.code {
                KeyCode::Down => {
                    self.ac_selected =
                        (self.ac_selected + 1).min(self.suggestions.len().saturating_sub(1));
                    return;
                }
                KeyCode::Up => {
                    self.ac_selected = self.ac_selected.saturating_sub(1);
                    return;
                }
                KeyCode::Enter => {
                    self.accept_suggestion();
                    return;
                }
                KeyCode::Esc => {
                    self.suggestions.clear();
                    return;
                }
                _ => {
                    self.suggestions.clear();
                }
            }
        }

        // Tab/BackTab cycles focus between fields.
        if key.code == KeyCode::Tab {
            self.suggestions.clear();
            if self.focus != Focus::Body || matches!(self.body_state.mode, EditorMode::Normal) {
                self.focus = match self.focus {
                    Focus::To => Focus::Cc,
                    Focus::Cc => Focus::Subject,
                    Focus::Subject => Focus::Body,
                    Focus::Body => Focus::To,
                };
                return;
            }
        }
        if key.code == KeyCode::BackTab {
            self.focus = match self.focus {
                Focus::To => Focus::Body,
                Focus::Cc => Focus::To,
                Focus::Subject => Focus::Cc,
                Focus::Body => Focus::Subject,
            };
            return;
        }

        match self.focus {
            Focus::To => self.to.on_key(key),
            Focus::Cc => self.cc.on_key(key),
            Focus::Subject => self.subject.on_key(key),
            Focus::Body => self.body_handler.on_key_event(key, &mut self.body_state),
        }
    }

    /// Reassemble the full draft text from header fields + body.
    pub fn text(&self) -> String {
        let mut draft = format!("To: {}\nSubject: {}\n", self.to.value, self.subject.value);
        if !self.cc.value.is_empty() {
            draft.push_str(&format!("Cc: {}\n", self.cc.value));
        }
        if let Some(ref irt) = self.in_reply_to {
            draft.push_str(&format!("In-Reply-To: {}\n", irt));
        }
        draft.push_str(&format!("{}\n", BODY_SENTINEL));
        draft.push_str(&self.body_state.lines.to_string());
        draft
    }

    /// Update autocomplete suggestions based on the focused field.
    pub fn update_autocomplete(&mut self, book: &AddressBook) {
        self.suggestions.clear();
        self.ac_selected = 0;

        let field = match self.focus {
            Focus::To => &self.to,
            Focus::Cc => &self.cc,
            _ => return,
        };

        let query = field.current_fragment();
        if query.len() < 2 {
            return;
        }

        self.suggestions = book
            .search(query)
            .into_iter()
            .take(8)
            .map(|a| a.full())
            .collect();
    }

    fn accept_suggestion(&mut self) {
        let Some(addr) = self.suggestions.get(self.ac_selected).cloned() else {
            return;
        };
        self.suggestions.clear();

        match self.focus {
            Focus::To => self.to.accept_suggestion(&addr),
            Focus::Cc => self.cc.accept_suggestion(&addr),
            _ => {}
        }
    }
}

/// Render the compose editor into `area`.
pub fn draw(frame: &mut ratatui::Frame, area: Rect, editor: &mut Editor) {
    let outer = Block::default().borders(Borders::NONE);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Layout: 3 header lines + 1 separator + body.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(inner);

    // Draw header fields.
    draw_header(frame, chunks[0], editor);

    // Draw body editor.
    draw_body(frame, chunks[1], editor);

    // Draw autocomplete dropdown overlay.
    draw_autocomplete(frame, chunks[0], editor);
}

fn draw_header(frame: &mut ratatui::Frame, area: Rect, editor: &Editor) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_field(frame, rows[0], &editor.to, editor.focus == Focus::To);
    draw_field(frame, rows[1], &editor.cc, editor.focus == Focus::Cc);
    draw_field(
        frame,
        rows[2],
        &editor.subject,
        editor.focus == Focus::Subject,
    );
}

fn draw_field(frame: &mut ratatui::Frame, area: Rect, field: &HeaderField, focused: bool) {
    let label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let label_w = field.label.len() as u16 + 2; // "To: "

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(label_w), Constraint::Min(0)])
        .split(area);

    let label = Paragraph::new(Span::styled(format!("{}: ", field.label), label_style));
    frame.render_widget(label, chunks[0]);

    if focused {
        // Show value with cursor.
        let before = &field.value[..field.cursor];
        let cursor_char = field.value[field.cursor..].chars().next().unwrap_or(' ');
        let after_len = field.cursor + cursor_char.len_utf8().min(field.value.len() - field.cursor);
        let after = &field.value[after_len..];

        let line = Line::from(vec![
            Span::raw(before),
            Span::styled(
                cursor_char.to_string(),
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::raw(after),
        ]);
        frame.render_widget(Paragraph::new(line), chunks[1]);
    } else {
        frame.render_widget(
            Paragraph::new(field.value.as_str()).style(Style::default()),
            chunks[1],
        );
    }
}

fn draw_body(frame: &mut ratatui::Frame, area: Rect, editor: &mut Editor) {
    // Highlight quoted lines ('>' prefix) in blue.
    let quote_style = Style::default().fg(Color::Blue);
    let mut highlights = Vec::new();
    for row in 0..editor.body_state.lines.len() {
        let Some(line) = editor.body_state.lines.get(RowIndex::new(row)) else {
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
    editor.body_state.set_highlights(highlights);

    let mode_style = match editor.body_state.mode {
        EditorMode::Normal => Style::default().bg(Color::Blue).fg(Color::Black),
        EditorMode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
        EditorMode::Visual => Style::default().bg(Color::Magenta).fg(Color::Black),
        _ => Style::default(),
    };

    let status_line = EditorStatusLine::default()
        .style_mode(mode_style.add_modifier(Modifier::BOLD))
        .style_line(Style::default().fg(Color::DarkGray));

    let mut theme = EditorTheme::default()
        .base(Style::default().bg(Color::Reset).fg(Color::Reset))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .selection_style(Style::default().add_modifier(Modifier::REVERSED))
        .line_numbers_style(Style::default().fg(Color::DarkGray))
        .status_line(status_line);

    if editor.focus == Focus::Body {
        theme =
            theme.cursor_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
    } else {
        theme = theme.hide_cursor();
    }

    let view = EditorView::new(&mut editor.body_state)
        .theme(theme)
        .wrap(true);
    frame.render_widget(view, area);

    // 80-column guide: highlight column 80 with a subtle background (no character, copy-safe)
    if area.width > 80 {
        let lines: Vec<Line> = std::iter::repeat_n(Line::from(" "), area.height as usize).collect();
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(Color::DarkGray)),
            Rect::new(area.x + 80, area.y, 1, area.height),
        );
    }
}

fn draw_autocomplete(frame: &mut ratatui::Frame, header_area: Rect, editor: &Editor) {
    if editor.suggestions.is_empty() {
        return;
    }

    // Position below the focused field.
    let field_row = match editor.focus {
        Focus::To => 0,
        Focus::Cc => 1,
        _ => return,
    };

    let dropdown_y = header_area.y + field_row as u16 + 1;
    let label_w = match editor.focus {
        Focus::To => 4,
        Focus::Cc => 4,
        _ => 4,
    };
    let dropdown_x = header_area.x + label_w;
    let dropdown_w = 44u16.min(header_area.width.saturating_sub(label_w));
    let dropdown_h = (editor.suggestions.len() as u16 + 2).min(10);

    let dropdown_area = Rect::new(dropdown_x, dropdown_y, dropdown_w, dropdown_h);

    frame.render_widget(Clear, dropdown_area);

    let items: Vec<ListItem> = editor
        .suggestions
        .iter()
        .map(|s| ListItem::new(s.as_str()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(editor.ac_selected));

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Addresses "))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED));

    frame.render_stateful_widget(list, dropdown_area, &mut list_state);
}
