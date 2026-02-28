use crate::cache::{EmailMeta, EmailThread, MailCache};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use std::io;
use std::rc::Rc;
use time::OffsetDateTime;
use time::macros::format_description;

// ── helpers ─────────────────────────────────────────────────────────────────

/// Format a timestamp from UTC tick to readably format.
fn format_timestamp(ts: Option<i64>) -> String {
    const DATE_WIDTH: usize = 17; // "YYYY-MM-DD HH:MM" + 1 space

    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]");
    match ts.and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok()) {
        Some(dt) => format!("{:<DATE_WIDTH$}", dt.format(fmt).unwrap_or_default()),
        None => format!("{:<DATE_WIDTH$}", "—"),
    }
}

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

/// Like `highlight_line` but takes an owned `String` and returns `Line<'static>`.
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

/// Format an address list (From / To / Cc) into a single comma-separated string.
/// Returns `"—"` when the header is absent.
fn format_addr_list(list: Option<&mail_parser::Address>) -> String {
    list.map(|addrs| {
        addrs
            .iter()
            .map(|a| {
                let name = a.name().unwrap_or_default();
                let addr = a.address().unwrap_or_default();
                if name.is_empty() {
                    addr.to_string()
                } else {
                    format!("{} <{}>", name, addr)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_else(|| "—".to_string())
}

// ── flat list entry ───────────────────────────────────────────────────────────

struct Entry {
    depth: usize,
    thread: Rc<EmailThread>,
}

fn flatten(threads: &[Rc<EmailThread>], depth: usize, out: &mut Vec<Entry>) {
    for thread in threads {
        out.push(Entry {
            depth,
            thread: thread.clone(),
        });
        let replies = thread.replies.borrow();
        flatten(&replies, depth + 1, out);
    }
}

// ── tab kinds ─────────────────────────────────────────────────────────────────

struct ListTab {
    list_state: ListState,
}

struct EmailTab {
    title: String,
    from: String,
    to: String,
    cc: String,
    date: String,
    body: String,
    scroll: u16,
    scroll_max: u16,
}

impl EmailTab {
    fn from_meta(meta: &EmailMeta) -> Result<Self> {
        let msg = MailCache::load_mail(meta)?;

        let from = format_addr_list(msg.from());
        let to = format_addr_list(msg.to());
        let cc = format_addr_list(msg.cc());

        let date = format_timestamp(meta.timestamp);

        let body = msg
            .body_text(0)
            .map(|t| t.into_owned())
            .unwrap_or_else(|| "— no text body —".to_string());

        let title = if meta.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            meta.subject.clone()
        };

        Ok(Self {
            title,
            from,
            to,
            cc,
            date,
            body,
            scroll: 0,
            scroll_max: u16::MAX,
        })
    }
}

// ── app state ─────────────────────────────────────────────────────────────────

struct App {
    list: ListTab,
    emails: Vec<EmailTab>,
    active: usize,
}

impl App {
    fn new(entries: &[Entry]) -> Self {
        let mut list_state = ListState::default();
        if !entries.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            list: ListTab { list_state },
            emails: Vec::new(),
            active: 0,
        }
    }

    fn tab_count(&self) -> usize {
        1 + self.emails.len()
    }

    fn go_left(&mut self) {
        if self.active > 0 {
            self.active -= 1;
        }
    }

    fn go_right(&mut self) {
        if self.active + 1 < self.tab_count() {
            self.active += 1;
        }
    }

    fn close_active(&mut self) {
        if self.active == 0 {
            return;
        }
        self.emails.remove(self.active - 1);
        self.active = self.active.min(self.tab_count() - 1);
    }
}

// ── public entry point ────────────────────────────────────────────────────────

pub fn run(cache: &MailCache) -> Result<()> {
    let mut entries: Vec<Entry> = Vec::new();
    flatten(&cache.threads, 0, &mut entries);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &entries);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ── main loop ─────────────────────────────────────────────────────────────────

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    entries: &[Entry],
) -> Result<()> {
    let mut app = App::new(entries);

    loop {
        terminal.draw(|frame| draw(frame, &mut app, entries))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    app.go_left();
                    continue;
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    app.go_right();
                    continue;
                }
                _ => {}
            }

            if app.active == 0 {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => {
                        if let Some(i) = app.list.list_state.selected() {
                            app.list
                                .list_state
                                .select(Some((i + 1).min(entries.len().saturating_sub(1))));
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if let Some(i) = app.list.list_state.selected() {
                            app.list.list_state.select(Some(i.saturating_sub(1)));
                        }
                    }
                    KeyCode::Home => app.list.list_state.select(Some(0)),
                    KeyCode::End => {
                        app.list
                            .list_state
                            .select(Some(entries.len().saturating_sub(1)));
                    }
                    KeyCode::Enter => {
                        if let Some(i) = app.list.list_state.selected() {
                            if let Ok(tab) = EmailTab::from_meta(&entries[i].thread.data) {
                                app.emails.push(tab);
                                app.active = app.tab_count() - 1;
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                let ei = app.active - 1;
                match key.code {
                    KeyCode::Char('q') => app.close_active(),
                    KeyCode::Esc => app.active = 0,

                    // Navigate to previous email in the list, replacing this tab.
                    KeyCode::Char('K') => {
                        if let Some(i) = app.list.list_state.selected() {
                            let prev = i.saturating_sub(1);
                            app.list.list_state.select(Some(prev));
                            if let Ok(new_tab) = EmailTab::from_meta(&entries[prev].thread.data) {
                                app.emails[ei] = new_tab;
                            }
                        }
                    }

                    // Navigate to next email in the list, replacing this tab.
                    KeyCode::Char('J') => {
                        if let Some(i) = app.list.list_state.selected() {
                            let next = (i + 1).min(entries.len().saturating_sub(1));
                            app.list.list_state.select(Some(next));
                            if let Ok(new_tab) = EmailTab::from_meta(&entries[next].thread.data) {
                                app.emails[ei] = new_tab;
                            }
                        }
                    }

                    // Line scrolling.
                    KeyCode::Char('j') | KeyCode::Down => {
                        let tab = &mut app.emails[ei];
                        tab.scroll = tab.scroll.saturating_add(1).min(tab.scroll_max);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.emails[ei].scroll = app.emails[ei].scroll.saturating_sub(1);
                    }

                    // Jump to first / last line.
                    KeyCode::Char('g') => {
                        app.emails[ei].scroll = 0;
                    }
                    KeyCode::Char('G') => {
                        let max = app.emails[ei].scroll_max;
                        app.emails[ei].scroll = max;
                    }

                    // Half-page scrolling (15 lines).
                    KeyCode::PageUp => {
                        app.emails[ei].scroll = app.emails[ei].scroll.saturating_sub(15);
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.emails[ei].scroll = app.emails[ei].scroll.saturating_sub(15);
                    }
                    KeyCode::PageDown => {
                        let tab = &mut app.emails[ei];
                        tab.scroll = tab.scroll.saturating_add(15).min(tab.scroll_max);
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let tab = &mut app.emails[ei];
                        tab.scroll = tab.scroll.saturating_add(15).min(tab.scroll_max);
                    }

                    KeyCode::Home => app.emails[ei].scroll = 0,

                    _ => {}
                }
            }
        }
    }

    Ok(())
}

// ── draw ──────────────────────────────────────────────────────────────────────

fn draw(frame: &mut ratatui::Frame, app: &mut App, entries: &[Entry]) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    // Tab bar.
    let mut titles: Vec<Line> = vec![Line::from(Span::raw("Threads"))];
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

    // Content + status bar.
    if app.active == 0 {
        draw_list(frame, entries, &mut app.list.list_state, chunks[1]);
        let selected = app.list.list_state.selected().map(|i| i + 1).unwrap_or(0);
        let status = Paragraph::new(format!(
            " {}/{} — j/k ↑/↓ move  Enter open  h/l ←/→ switch tabs  q quit",
            selected,
            entries.len()
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    } else {
        let ei = app.active - 1;
        draw_email(frame, &mut app.emails[ei], chunks[1]);
        let status = Paragraph::new(
            " j/k ↑/↓ scroll  h/l ←/→ switch tabs  J/K select email  Esc back to list  q close tab",
        )
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    }
}

fn draw_list(
    frame: &mut ratatui::Frame,
    entries: &[Entry],
    list_state: &mut ListState,
    area: ratatui::layout::Rect,
) {
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
                "(no subject)"
            } else {
                e.thread.data.subject.as_str()
            };
            ListItem::new(Line::from(vec![
                Span::styled(date, Style::default().fg(Color::Cyan)),
                Span::styled(indent, Style::default().fg(Color::DarkGray)),
                Span::raw(subject.to_string()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Threads "))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, list_state);
}

fn draw_email(frame: &mut ratatui::Frame, tab: &mut EmailTab, area: ratatui::layout::Rect) {
    // inner_width: area minus left/right borders, minus the "From : " label width.
    const LABEL: usize = 7; // "From : " / "To   : " / "Cc   : " / "Date : "
    let inner_width = area.width.saturating_sub(2) as usize; // subtract borders
    let value_width = inner_width.saturating_sub(LABEL).max(1);

    let from_lines = wrap_header_field("From : ", &tab.from, value_width);
    let to_lines = wrap_header_field("To   : ", &tab.to, value_width);
    let cc_lines = wrap_header_field("Cc   : ", &tab.cc, value_width);

    // +2 for top/bottom borders, +1 for Date which is always one line
    let header_height = (from_lines.len() + to_lines.len() + cc_lines.len() + 1 + 2) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(0)])
        .split(area);

    // Header block — each field wraps independently.
    let mut header_text: Vec<Line> = Vec::new();
    header_text.extend(from_lines);
    header_text.extend(to_lines);
    header_text.extend(cc_lines);
    header_text.push(Line::from(vec![
        Span::styled("Date : ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(tab.date.trim()),
    ]));

    let header = Paragraph::new(header_text).block(Block::default().borders(Borders::ALL).title(
        Span::styled(
            format!(" {} ", truncate(&tab.title, 60)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ));
    frame.render_widget(header, chunks[0]);

    // Body with diff highlighting.
    let body_lines = highlight_body(&tab.body);

    // Compute how many terminal rows the wrapped body occupies, then derive
    // the maximum scroll offset so the last line never scrolls past the bottom.
    let inner_width = chunks[1].width.saturating_sub(2) as usize; // subtract borders
    let visible_height = chunks[1].height.saturating_sub(2) as usize; // subtract borders
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

/// Wrap a header field value into one or more `Line`s.
///
/// The label (e.g. `"From : "`) appears on the first line, and continuation
/// lines are indented by the same width so values stay visually aligned.
fn wrap_header_field<'a>(label: &'a str, value: &'a str, value_width: usize) -> Vec<Line<'a>> {
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut chars = value.chars();
    let mut lines: Vec<Line<'a>> = Vec::new();

    // First line: label + up to value_width chars of the value.
    let first: String = chars.by_ref().take(value_width).collect();
    lines.push(Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(first),
    ]));

    // Continuation lines indented to align under the value.
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
