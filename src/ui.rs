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
use std::io::{self, Write};
use std::process::Command;
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

struct EmailTab {
    title: String,
    from: String,
    to: String,
    cc: String,
    date: String,
    body: String,
    message_id: Option<String>,
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

        let message_id = msg.message_id().map(|s| s.to_owned());

        Ok(Self {
            title,
            from,
            to,
            cc,
            date,
            body,
            message_id,
            scroll: 0,
            scroll_max: u16::MAX,
        })
    }
}

// ── app state ─────────────────────────────────────────────────────────────────

struct App {
    /// Which top-level tab is shown: 0 = list view, 1+ = open email tabs.
    active: usize,
    emails: Vec<EmailTab>,
    /// Index of the currently highlighted mailbox in the left pane.
    selected_mailbox: usize,
    mailbox_list_state: ListState,
    /// Per-mailbox thread list state (selection within threads).
    thread_list_states: Vec<ListState>,
}

impl App {
    fn new(mailbox_count: usize) -> Self {
        let mut mailbox_list_state = ListState::default();
        if mailbox_count > 0 {
            mailbox_list_state.select(Some(0));
        }
        let mut thread_list_states: Vec<ListState> = (0..mailbox_count)
            .map(|_| {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            })
            .collect();
        // Don't select anything if there are no threads (handled at draw time).
        let _ = thread_list_states.first_mut();
        Self {
            active: 0,
            emails: Vec::new(),
            selected_mailbox: 0,
            mailbox_list_state,
            thread_list_states,
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

    /// The currently selected thread index within the active mailbox.
    fn selected_thread(&self) -> Option<usize> {
        self.thread_list_states[self.selected_mailbox].selected()
    }

    /// Move thread selection down within the active mailbox.
    fn thread_down(&mut self, mailbox_entries: &[Vec<Entry>]) {
        let len = mailbox_entries[self.selected_mailbox].len();
        if len == 0 {
            return;
        }
        let cur = self.thread_list_states[self.selected_mailbox]
            .selected()
            .unwrap_or(0);
        self.thread_list_states[self.selected_mailbox].select(Some((cur + 1).min(len - 1)));
    }

    /// Move thread selection up within the active mailbox.
    fn thread_up(&mut self) {
        let cur = self.thread_list_states[self.selected_mailbox]
            .selected()
            .unwrap_or(0);
        self.thread_list_states[self.selected_mailbox].select(Some(cur.saturating_sub(1)));
    }

    /// Move thread selection to first thread.
    fn thread_home(&mut self) {
        self.thread_list_states[self.selected_mailbox].select(Some(0));
    }

    /// Move thread selection to last thread.
    fn thread_end(&mut self, mailbox_entries: &[Vec<Entry>]) {
        let len = mailbox_entries[self.selected_mailbox].len();
        if len > 0 {
            self.thread_list_states[self.selected_mailbox].select(Some(len - 1));
        }
    }

    /// Switch to the next mailbox.
    fn mailbox_down(&mut self, count: usize) {
        if self.selected_mailbox + 1 < count {
            self.selected_mailbox += 1;
            self.mailbox_list_state.select(Some(self.selected_mailbox));
        }
    }

    /// Switch to the previous mailbox.
    fn mailbox_up(&mut self) {
        if self.selected_mailbox > 0 {
            self.selected_mailbox -= 1;
            self.mailbox_list_state.select(Some(self.selected_mailbox));
        }
    }
}

// ── public entry point ────────────────────────────────────────────────────────

pub fn run(mailboxes: &[(&str, MailCache)]) -> Result<()> {
    let labels: Vec<&str> = mailboxes.iter().map(|(l, _)| *l).collect();
    let entries: Vec<Vec<Entry>> = mailboxes
        .iter()
        .map(|(_, cache)| {
            let mut v = Vec::new();
            flatten(&cache.threads, 0, &mut v);
            v
        })
        .collect();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &labels, &entries);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ── reply ────────────────────────────────────────────────────────────────────

/// Extract a bare email address from a display string like "Name <addr>" or "addr".
fn bare_address(s: &str) -> &str {
    if let Some(start) = s.find('<') {
        if let Some(end) = s[start..].find('>') {
            return s[start + 1..start + end].trim();
        }
    }
    s.trim()
}

/// Build a reply draft, open vim, ask for confirmation, then send via sendmail.
///
/// `quote` — when true the original body is included quoted with "> ".
/// TUI is suspended while vim has the terminal, then restored for the dialog.
fn reply(
    tab: &EmailTab,
    quote: bool,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let to = bare_address(&tab.from);
    let subject = if tab.title.starts_with("Re:") || tab.title.starts_with("re:") {
        tab.title.clone()
    } else {
        format!("Re: {}", tab.title)
    };

    // Build the editor draft:
    //   <headers>
    //   --
    //   <blank line — user types reply here>
    //   <optional quoted body>
    //
    // The "--" is a visible separator in the editor only; before sending it is
    // replaced with a blank line to produce a valid RFC 2822 message.
    let mut header_count = 2usize; // To + Subject
    let mut draft = String::new();
    draft.push_str(&format!("To: {}\n", to));
    draft.push_str(&format!("Subject: {}\n", subject));
    if let Some(ref mid) = tab.message_id {
        draft.push_str(&format!("In-Reply-To: <{}>\n", mid));
        header_count += 1;
    }
    draft.push_str("--\n"); // visible separator
    draft.push('\n'); // blank reply line — cursor lands here
    if quote {
        for line in tab.body.lines() {
            draft.push_str(&format!("> {}\n", line));
        }
    }

    // Write to a temp file.
    let tmp_path = std::env::temp_dir().join(format!("mail-reply-{}.eml", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(draft.as_bytes())?;
    }

    // Suspend TUI, open vim with cursor on the blank reply line.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    // cursor on the blank reply line: header_count + "--" line + blank line (1-based)
    let reply_line = header_count + 2;
    Command::new(&editor)
        .arg(format!("+{}", reply_line))
        .arg(&tmp_path)
        .status()?;

    // Read back the edited file.
    let edited = std::fs::read_to_string(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    // Restore TUI.
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;

    // Ask for confirmation before sending.
    if !confirm_send(terminal)? {
        return Ok(());
    }

    // Replace the "--\n" separator with a blank line to produce a valid
    // RFC 2822 message (headers \n\n body) that sendmail -t understands.
    let message = edited.replacen("--\n", "\n", 1);

    // Send via sendmail -t (reads recipients from To:/Cc:/Bcc: headers).
    let sendmail = std::env::var("SENDMAIL").unwrap_or_else(|_| "sendmail".to_string());
    let mut child = Command::new(&sendmail)
        .arg("-t")
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(message.as_bytes())?;
    }

    child.wait()?;

    Ok(())
}

/// Draw a centred confirmation dialog and wait for y / n / Esc / Enter.
/// Returns `true` if the user confirms (y / Enter), `false` otherwise.
fn confirm_send(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();

            // Centre a 36×5 popup.
            let popup_w: u16 = 36;
            let popup_h: u16 = 5;
            let x = area.width.saturating_sub(popup_w) / 2;
            let y = area.height.saturating_sub(popup_h) / 2;
            let popup_area =
                ratatui::layout::Rect::new(x, y, popup_w.min(area.width), popup_h.min(area.height));

            use ratatui::widgets::Clear;
            frame.render_widget(Clear, popup_area);

            let block = Block::default().borders(Borders::ALL).title(Span::styled(
                " Send reply? ",
                Style::default().add_modifier(Modifier::BOLD),
            ));

            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);

            let text = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "  [ y ]",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  send    "),
                    Span::styled(
                        "[ n / Esc ]",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  cancel"),
                ]),
            ];
            frame.render_widget(Paragraph::new(text), inner);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }
}

// ── main loop ─────────────────────────────────────────────────────────────────

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    labels: &[&str],
    mailbox_entries: &[Vec<Entry>],
) -> Result<()> {
    let mut app = App::new(labels.len());

    loop {
        terminal.draw(|frame| draw(frame, &mut app, labels, mailbox_entries))?;

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
                // ── list view ──
                let entries = &mailbox_entries[app.selected_mailbox];
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => app.thread_down(mailbox_entries),
                    KeyCode::Char('k') | KeyCode::Up => app.thread_up(),
                    KeyCode::Home => app.thread_home(),
                    KeyCode::End => app.thread_end(mailbox_entries),
                    // J/K switch mailbox.
                    KeyCode::Char('J') => app.mailbox_down(labels.len()),
                    KeyCode::Char('K') => app.mailbox_up(),
                    KeyCode::Enter => {
                        if let Some(ti) = app.selected_thread() {
                            if let Ok(tab) = EmailTab::from_meta(&entries[ti].thread.data) {
                                app.emails.push(tab);
                                app.active = app.tab_count() - 1;
                            }
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Some(ti) = app.selected_thread() {
                            if let Ok(tab) = EmailTab::from_meta(&entries[ti].thread.data) {
                                let _ = reply(&tab, true, terminal);
                            }
                        }
                    }
                    KeyCode::Char('R') => {
                        if let Some(ti) = app.selected_thread() {
                            if let Ok(tab) = EmailTab::from_meta(&entries[ti].thread.data) {
                                let _ = reply(&tab, false, terminal);
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                // ── email tab view ──
                let ei = app.active - 1;
                match key.code {
                    KeyCode::Char('q') => app.close_active(),
                    KeyCode::Esc => app.active = 0,
                    KeyCode::Char('r') => {
                        let _ = reply(&app.emails[ei], true, terminal);
                    }
                    KeyCode::Char('R') => {
                        let _ = reply(&app.emails[ei], false, terminal);
                    }
                    // J/K navigate to next/prev thread in the active mailbox.
                    KeyCode::Char('J') => {
                        app.thread_down(mailbox_entries);
                        let entries = &mailbox_entries[app.selected_mailbox];
                        if let Some(ti) = app.selected_thread() {
                            if let Ok(new_tab) = EmailTab::from_meta(&entries[ti].thread.data) {
                                app.emails[ei] = new_tab;
                            }
                        }
                    }
                    KeyCode::Char('K') => {
                        app.thread_up();
                        let entries = &mailbox_entries[app.selected_mailbox];
                        if let Some(ti) = app.selected_thread() {
                            if let Ok(new_tab) = EmailTab::from_meta(&entries[ti].thread.data) {
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
                    KeyCode::Char('g') => {
                        app.emails[ei].scroll = 0;
                    }
                    KeyCode::Char('G') => {
                        let max = app.emails[ei].scroll_max;
                        app.emails[ei].scroll = max;
                    }
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
                    KeyCode::Home => {
                        app.emails[ei].scroll = 0;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

// ── draw ──────────────────────────────────────────────────────────────────────

fn draw(
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

    // ── content + status bar ──
    if app.active == 0 {
        draw_list(frame, app, labels, mailbox_entries, chunks[1]);
        let entries = &mailbox_entries[app.selected_mailbox];
        let selected = app.selected_thread().map(|i| i + 1).unwrap_or(0);
        let status = Paragraph::new(format!(
            " {}/{} — j/k move  J/K mailbox  Enter open  r reply  R reply-empty  h/l tabs  q quit",
            selected,
            entries.len(),
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    } else {
        let ei = app.active - 1;
        draw_email(frame, &mut app.emails[ei], chunks[1]);
        let status = Paragraph::new(
            " j/k scroll  J/K thread  h/l tabs  r reply  R reply-empty  Esc back  q close",
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
) {
    // Split horizontally: left pane for mailboxes, right pane for threads.
    // Left pane width = longest label + 2 borders + 2 padding, min 16, max 32.
    let left_w = (labels.iter().map(|l| l.len()).max().unwrap_or(8) + 4).clamp(16, 32) as u16;
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_w), Constraint::Min(0)])
        .split(area);

    // ── left: mailbox list ──
    let mb_items: Vec<ListItem> = labels
        .iter()
        .map(|l| ListItem::new(Line::from(Span::raw(l.to_string()))))
        .collect();
    let mb_list = List::new(mb_items)
        .block(Block::default().borders(Borders::ALL).title(" Mailboxes "))
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
            ListItem::new(Line::from(vec![
                Span::styled(date, Style::default().fg(Color::Cyan)),
                Span::styled(indent, Style::default().fg(Color::DarkGray)),
                Span::raw(subject),
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
