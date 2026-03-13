// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::address::Address;
use crate::core::thread::Email;
use anyhow::Result;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use time::OffsetDateTime;
use time::macros::format_description;

pub struct EmailView {
    path: std::path::PathBuf,
    message_id: String,
    subject: String,
    from: Vec<Address>,
    to: Vec<Address>,
    cc: Vec<Address>,
    date: String,
    body_lines: Vec<Line<'static>>,
    scroll: u16,
    raw_body: String,
}

impl EmailView {
    pub fn new(email: &Email) -> Result<Self> {
        let msg = email.to_message()?;

        let message_id;
        if let Some(id) = msg.message_id() {
            if id.is_empty() {
                return Err(anyhow::anyhow!("Message-ID is empty"));
            }

            message_id = id.to_string();
        } else {
            return Err(anyhow::anyhow!("Message-ID is None"));
        }

        let from = parse_addr_list(msg.from());
        let to = parse_addr_list(msg.to());
        let cc = parse_addr_list(msg.cc());
        let date = format_date(email.timestamp);
        let raw_body = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
        let display_body = if raw_body.is_empty() {
            "— no text body —".to_string()
        } else {
            raw_body.clone()
        };
        let body_lines = highlight_body(&display_body);
        let subject = if email.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            email.subject.clone()
        };

        Ok(Self {
            path: email.path().to_path_buf(),
            message_id,
            subject,
            from,
            to,
            cc,
            date,
            body_lines,
            scroll: 0,
            raw_body,
        })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn scroll_up(&mut self, steps: u16) {
        self.scroll = self.scroll.saturating_sub(steps);
    }

    pub fn scroll_down(&mut self, steps: u16) {
        let max = self.body_lines.len().saturating_sub(1) as u16;
        self.scroll = self.scroll.saturating_add(steps).min(max);
    }

    pub fn first_line(&mut self) {
        self.scroll = 0;
    }

    pub fn last_line(&mut self) {
        self.scroll = self
            .scroll
            .saturating_add(self.body_lines.len().saturating_sub(1) as u16);
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn message_id(&self) -> &str {
        &self.message_id
    }

    pub fn raw_body(&self) -> &str {
        &self.raw_body
    }
}

/// Render the email view into `area`.
///
/// The area is split into a header block (From/To/Cc/Date) and a scrollable
/// body block below it.
pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, view: &mut EmailView) {
    const LABEL: usize = 7;
    let inner_width = area.width as usize;
    let value_width = inner_width.saturating_sub(LABEL).max(1);

    let from_str = format_addr_list(&view.from);
    let to_str = format_addr_list(&view.to);
    let cc_str = format_addr_list(&view.cc);
    let from_lines = wrap_header_field("From : ", &from_str, value_width);
    let to_lines = wrap_header_field("To   : ", &to_str, value_width);
    let cc_lines = wrap_header_field("Cc   : ", &cc_str, value_width);
    let subject_lines = wrap_header_field("Subj : ", view.subject(), value_width);

    let header_height =
        (from_lines.len() + to_lines.len() + cc_lines.len() + subject_lines.len() + 1 + 1) as u16;

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
        Span::raw(view.date.clone()),
    ]));
    header_text.extend(subject_lines);

    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, chunks[0]);

    // Calculate viewport height
    let body_height = chunks[1].height as usize;
    let max_scroll = view.body_lines.len().saturating_sub(body_height) as u16;

    // Clamp scroll so last line stays at bottom when content fits
    view.scroll = view.scroll.min(max_scroll);

    let body = Paragraph::new(view.body_lines.to_vec())
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false })
        .scroll((view.scroll, 0));
    frame.render_widget(body, chunks[1]);
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Parse a `mail_parser` address header into a list of `Address` values.
fn parse_addr_list(list: Option<&mail_parser::Address>) -> Vec<Address> {
    list.map(|addrs| {
        addrs
            .iter()
            .map(|a| {
                Address::new(
                    a.name().unwrap_or_default(),
                    a.address().unwrap_or_default(),
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Format a list of addresses into a comma-separated display string.
fn format_addr_list(addrs: &[Address]) -> String {
    if addrs.is_empty() {
        "—".to_string()
    } else {
        addrs
            .iter()
            .map(|a| a.full())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn format_date(ts: Option<i64>) -> String {
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]");
    match ts.and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok()) {
        Some(dt) => dt.format(fmt).unwrap_or_else(|_| "—".to_string()),
        None => "—".to_string(),
    }
}

fn highlight_body(body: &str) -> Vec<Line<'static>> {
    body.lines()
        .map(|raw| highlight_line_owned(expand_tabs(raw)))
        .collect()
}

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

fn highlight_line_owned(raw: String) -> Line<'static> {
    let style = if raw.starts_with("---") {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if raw.starts_with("+++") {
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

fn wrap_header_field<'a>(label: &'a str, value: &'a str, value_width: usize) -> Vec<Line<'a>> {
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut chars = value.chars();

    let first: String = chars.by_ref().take(value_width).collect();
    let mut lines = vec![Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(first),
    ])];

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

#[cfg(test)]
impl EmailView {
    pub(crate) fn new_stub(subject: &str) -> Self {
        Self {
            path: std::path::PathBuf::new(),
            message_id: "stub@test".to_string(),
            subject: subject.to_string(),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date: String::new(),
            body_lines: Vec::new(),
            scroll: 0,
            raw_body: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::thread::Email;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("brew_email2_test_{}", id));
        std::fs::create_dir_all(dir.join("new")).unwrap();
        dir
    }

    fn write_email(dir: &PathBuf, name: &str, content: &str) -> PathBuf {
        let path = dir.join("new").join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn full_email(from: &str, to: &str, cc: &str, subject: &str, body: &str) -> String {
        format!(
            "Message-ID: <test@example.com>\r\nFrom: {from}\r\nTo: {to}\r\nCc: {cc}\r\nSubject: {subject}\r\nDate: Mon, 01 Jan 2024 12:00:00 +0000\r\n\r\n{body}"
        )
    }

    fn make_view(dir: &PathBuf, content: &str) -> EmailView {
        let path = write_email(dir, "msg", content);
        let email = Email::from_file(&path).unwrap();
        EmailView::new(&email).unwrap()
    }

    fn rendered_lines(view: &mut EmailView, w: u16, h: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, ratatui::layout::Rect::new(0, 0, w, h), view))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()))
                    .collect()
            })
            .collect()
    }

    // ── subject ──────────────────────────────────────────────────────────────

    #[test]
    fn subject_returns_subject() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "My Topic", "body"),
        );
        assert_eq!(view.subject(), "My Topic");
    }

    #[test]
    fn subject_returns_placeholder_when_empty() {
        let dir = temp_dir();
        let path = write_email(
            &dir,
            "msg",
            "Message-ID: <x@x>\r\nFrom: a@x.com\r\nTo: b@x.com\r\nDate: Mon, 01 Jan 2024 12:00:00 +0000\r\n\r\nbody",
        );
        let email = Email::from_file(&path).unwrap();
        let view = EmailView::new(&email).unwrap();
        assert_eq!(view.subject(), "(no subject)");
    }

    // ── new ──────────────────────────────────────────────────────────────────

    #[test]
    fn new_stores_message_id() {
        let dir = temp_dir();
        let path = write_email(
            &dir,
            "msg",
            "Message-ID: <unique-id@example.com>\r\nFrom: a@x.com\r\nTo: b@x.com\r\nDate: Mon, 01 Jan 2024 12:00:00 +0000\r\n\r\nbody",
        );
        let email = Email::from_file(&path).unwrap();
        let view = EmailView::new(&email).unwrap();
        assert_eq!(view.message_id(), "unique-id@example.com");
    }

    #[test]
    fn new_parses_from() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email(
                "Alice <alice@example.com>",
                "bob@example.com",
                "",
                "Hi",
                "body",
            ),
        );
        let from = format_addr_list(&view.from);
        assert!(from.contains("Alice"), "got: {}", from);
    }

    #[test]
    fn new_parses_to() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email(
                "alice@example.com",
                "Bob <bob@example.com>",
                "",
                "Hi",
                "body",
            ),
        );
        let to = format_addr_list(&view.to);
        assert!(to.contains("Bob"), "got: {}", to);
    }

    #[test]
    fn new_parses_cc() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "Carol <carol@x.com>", "Hi", "body"),
        );
        let cc = format_addr_list(&view.cc);
        assert!(cc.contains("Carol"), "got: {}", cc);
    }

    #[test]
    fn new_uses_subject_as_title() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "My Subject", "body"),
        );
        assert_eq!(view.subject, "My Subject");
    }

    #[test]
    fn new_formats_date() {
        let dir = temp_dir();
        let view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", "body"));
        assert_eq!(view.date, "2024-01-01 12:00");
    }

    #[test]
    fn new_missing_date_formats_dash() {
        let dir = temp_dir();
        let path = write_email(
            &dir,
            "msg",
            "Message-ID: <x@x>\r\nFrom: a@x.com\r\nTo: b@x.com\r\nSubject: Hi\r\n\r\nbody",
        );
        let email = Email::from_file(&path).unwrap();
        let view = EmailView::new(&email).unwrap();
        assert_eq!(view.date, "—");
    }

    #[test]
    fn new_populates_body_lines() {
        let dir = temp_dir();
        let view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "Hi", "line one\nline two"),
        );
        assert!(!view.body_lines.is_empty());
    }

    #[test]
    fn new_no_body_uses_placeholder() {
        let dir = temp_dir();
        let path = write_email(
            &dir,
            "msg",
            "Message-ID: <x@x>\r\nFrom: a@x.com\r\nTo: b@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 12:00:00 +0000\r\n\r\n",
        );
        let email = Email::from_file(&path).unwrap();
        let view = EmailView::new(&email).unwrap();
        let text: String = view
            .body_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("no text body"), "got: {text}");
    }

    #[test]
    fn new_bad_path_returns_error() {
        use std::path::PathBuf;
        let email = Email::new(
            "x",
            None,
            "",
            "",
            None,
            PathBuf::from("/nonexistent/path/msg"),
        );
        assert!(EmailView::new(&email).is_err());
    }

    // ── scroll ────────────────────────────────────────────────────────────────

    #[test]
    fn scroll_up_at_zero_stays() {
        let dir = temp_dir();
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", "body"));
        view.scroll_up(1);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn scroll_down_capped_at_zero_when_no_max() {
        let dir = temp_dir();
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", "body"));
        view.scroll_down(1); // scroll_max is 0 by default
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn scroll_down_increments_within_max() {
        let dir = temp_dir();
        let body = (0..10).map(|i| format!("line {i}\n")).collect::<String>();
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", &body));
        view.scroll_down(3);
        assert_eq!(view.scroll, 3);
    }

    #[test]
    fn scroll_down_clamps_at_last_line() {
        let dir = temp_dir();
        // 3 body lines → max index = 2
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", "a\nb\nc"));
        view.scroll_down(100);
        assert_eq!(view.scroll, 2);
    }

    #[test]
    fn scroll_up_decrements_by_steps() {
        let dir = temp_dir();
        let body = (0..10).map(|i| format!("line {i}\n")).collect::<String>();
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", &body));
        view.scroll = 7;
        view.scroll_up(3);
        assert_eq!(view.scroll, 4);
    }

    #[test]
    fn scroll_up_clamps_at_zero() {
        let dir = temp_dir();
        let mut view = make_view(&dir, &full_email("a@x.com", "b@x.com", "", "Hi", "body"));
        view.scroll = 2;
        view.scroll_up(10);
        assert_eq!(view.scroll, 0);
    }

    // ── draw ─────────────────────────────────────────────────────────────────

    #[test]
    fn draw_shows_subject_in_title() {
        let dir = temp_dir();
        let mut view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "Important topic", "body"),
        );
        let content: String = rendered_lines(&mut view, 80, 20).join("\n");
        assert!(content.contains("Important topic"), "got:\n{content}");
    }

    #[test]
    fn draw_shows_from_header() {
        let dir = temp_dir();
        let mut view = make_view(
            &dir,
            &full_email("Alice <alice@example.com>", "b@x.com", "", "Hi", "body"),
        );
        let content: String = rendered_lines(&mut view, 80, 20).join("\n");
        assert!(content.contains("Alice"), "got:\n{content}");
    }

    #[test]
    fn draw_shows_body_content() {
        let dir = temp_dir();
        let mut view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "Hi", "Hello from the body"),
        );
        let content: String = rendered_lines(&mut view, 80, 20).join("\n");
        assert!(content.contains("Hello from the body"), "got:\n{content}");
    }

    #[test]
    fn draw_long_body_scrollable() {
        let dir = temp_dir();
        let long_body: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut view = make_view(
            &dir,
            &full_email("a@x.com", "b@x.com", "", "Hi", &long_body),
        );
        // scroll_down is limited to body_lines.len() - 1
        view.scroll_down(1000);
        assert!(view.scroll > 0);
        rendered_lines(&mut view, 80, 20); // must not panic with scroll set
    }
}
