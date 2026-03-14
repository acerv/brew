// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::address::{Address, AddressBook};
use crate::core::config::{self, Config, Smtp};
use crate::core::maildir::{Maildir, SortOrder};
use crate::core::thread::Email;
use crate::ui::compose::{self, EmailReply};
use crate::ui::editor::{self, Editor};
use crate::ui::email::{self, EmailView};
use crate::ui::threads::{self, ThreadsView};
use crate::ui::utils;
use anyhow::anyhow;
use arboard::Clipboard;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::EditorMode;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
};
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const POLLING_TIME: u64 = 30;

enum SearchMode {
    Off,
    Typing(String),
    Applied,
}

enum MoveMode {
    Off,
    Active { selected: usize },
}

enum Tab {
    Email(Box<EmailView>),
    Compose(Box<Editor>),
}

pub struct App {
    config: Config,
    sidebar_state: ListState,
    maildirs: Vec<Maildir>,
    threads: Vec<ThreadsView>,
    /// Open tabs. Tab 0 is always the "Brew" main view; tabs start at index 1.
    tabs: Vec<Tab>,
    current_tab: usize,
    current_mb: usize,
    pending_sync: Option<mpsc::Receiver<Option<String>>>,
    search: SearchMode,
    move_mode: MoveMode,
    status_error: Option<String>,
    terminal: Option<Terminal<CrosstermBackend<io::Stdout>>>,
    address_book: AddressBook,
    clipboard: Option<Clipboard>,
}

impl App {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let mut maildirs = Vec::new();
        let mut threads = Vec::new();
        let clipboard = Clipboard::new().ok();

        for mb in &config.mailboxes {
            let maildir = Maildir::new(&mb.path).unwrap_or_default();
            let tv = ThreadsView::new(maildir.threads());
            maildirs.push(maildir);
            threads.push(tv);
        }

        let mut address_book = AddressBook::load();
        let mut addrs = Vec::new();
        for md in &maildirs {
            for thread in md.threads().borrow().iter() {
                addrs.push(thread.parent.from.clone());
            }
        }
        address_book.harvest(&addrs);

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut sidebar_state = ListState::default();
        if !config.mailboxes.is_empty() {
            sidebar_state.select(Some(0));
        }

        Ok(Self {
            sidebar_state,
            config,
            maildirs,
            threads,
            tabs: Vec::new(),
            current_tab: 0,
            current_mb: 0,
            pending_sync: None,
            search: SearchMode::Off,
            move_mode: MoveMode::Off,
            status_error: None,
            terminal: Some(terminal),
            address_book,
            clipboard,
        })
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut last_sync = Instant::now();
        let interval = if let Some(sync) = &self.config.sync {
            let dur = Duration::from_secs(sync.interval);
            // Force the first sync at startup
            last_sync -= dur;
            Some(dur)
        } else {
            None
        };

        loop {
            // Check if pending sync completed (non-blocking)
            if let Some(ref rx) = self.pending_sync {
                match rx.try_recv() {
                    Ok(None) => {
                        // Success
                        for maildir in &mut self.maildirs {
                            maildir.sync();
                        }
                        for tv in &mut self.threads {
                            tv.invalidate();
                        }
                        self.status_error = None;
                        self.pending_sync = None;
                        last_sync = Instant::now();
                    }
                    Ok(Some(err)) => {
                        // Failed
                        self.status_error = Some(format!("sync: {}", err));
                        self.pending_sync = None;
                        last_sync = Instant::now();
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        // Still running, continue
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        // Thread panicked or dropped
                        self.pending_sync = None;
                        last_sync = Instant::now();
                    }
                }
            } else {
                if let Some(i) = interval
                    && last_sync.elapsed() >= i
                {
                    self.trigger_sync();
                }
            }

            let mut terminal = self.terminal.take().unwrap();
            terminal.draw(|frame| draw(frame, self))?;
            self.terminal = Some(terminal);

            if event::poll(Duration::from_secs(POLLING_TIME))?
                && let Event::Key(key) = event::read()?
            {
                if key.kind != event::KeyEventKind::Press {
                    continue;
                }
                if !self.handle_key(key) {
                    break;
                }
            }
        }

        if let Some(mut terminal) = self.terminal.take() {
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;
        }

        Ok(())
    }

    /// Handle a key event. Returns `false` when the app should quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                self.next_tab();
                return true;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                self.prev_tab();
                return true;
            }
            _ => {}
        }

        if matches!(self.search, SearchMode::Typing(_)) {
            self.handle_search_key(key);
            return true;
        }

        if matches!(self.move_mode, MoveMode::Active { .. }) {
            self.handle_move_key(key);
            return true;
        }

        if self.current_tab == 0 {
            self.handle_main_key(key)
        } else {
            self.handle_tab_key(key)
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> bool {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('Q')) => return false,
            (_, KeyCode::Char('f')) => self.trigger_sync(),
            (_, KeyCode::Char('D')) => self.delete_selected_thread(),
            (_, KeyCode::Char('N')) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.toggle_unread();
                }
            }
            (_, KeyCode::Char('s')) => {
                let idx = self.current_mb;
                if let Some(md) = self.maildirs.get_mut(idx) {
                    md.set_sort_order(md.sort_order().toggle());
                    if let Some(tv) = self.threads.get_mut(idx) {
                        tv.invalidate();
                    }
                }
            }
            (_, KeyCode::Char('J')) => self.next_mailbox(),
            (_, KeyCode::Char('K')) => self.prev_mailbox(),
            (_, KeyCode::Char('j') | KeyCode::Down) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.next_email(1);
                }
            }
            (_, KeyCode::Char('k') | KeyCode::Up) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.prev_email(1);
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) | (_, KeyCode::PageDown) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.next_email(15);
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) | (_, KeyCode::PageUp) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.prev_email(15);
                }
            }
            (_, KeyCode::Char('g')) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.first_email();
                }
            }
            (_, KeyCode::Char('G')) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.last_email();
                }
            }
            (_, KeyCode::Enter) => self.open_selected_email(),
            (_, KeyCode::Char('/')) => {
                self.search = SearchMode::Typing(String::new());
            }
            (_, KeyCode::Esc) => self.reset_search(),
            (_, KeyCode::Char('v')) => self.toggle_read(),
            (_, KeyCode::Char('m')) => {
                if self
                    .threads
                    .get(self.current_mb)
                    .and_then(|tv| tv.selected())
                    .is_some()
                    && self.config.mailboxes.len() > 1
                {
                    self.move_mode = MoveMode::Active { selected: 0 };
                }
            }
            (_, KeyCode::Char('C')) => self.compose(),
            (_, KeyCode::Char('r')) => self.open_reply_from_thread(false),
            (_, KeyCode::Char('R')) => self.open_reply_from_thread(true),
            _ => {}
        }
        true
    }

    fn handle_tab_key(&mut self, key: KeyEvent) -> bool {
        let ei = self.current_tab.saturating_sub(1);
        if ei >= self.tabs.len() {
            return true;
        }
        match self.tabs[ei] {
            Tab::Email(_) => self.handle_email_tab_key(key, ei),
            Tab::Compose(_) => self.handle_compose_tab_key(key, ei),
        }
        true
    }

    fn handle_compose_tab_key(&mut self, key: KeyEvent, ei: usize) {
        let should_show_dialog = if let Tab::Compose(ref mut ed) = self.tabs[ei] {
            if matches!(key.code, KeyCode::Char('q')) && ed.mode() == EditorMode::Normal {
                true
            } else {
                ed.on_key(key);
                ed.update_autocomplete(&self.address_book);
                false
            }
        } else {
            false
        };

        if !should_show_dialog {
            return;
        }

        // Extract text before borrowing terminal
        let text = if let Tab::Compose(ref ed) = self.tabs[ei] {
            ed.text()
        } else {
            return;
        };

        let drafts_idx = self.config.mailboxes.iter().position(|mb| mb.is_drafts());
        let mut action = SendAction::GoBack;
        if let Some(ref mut terminal) = self.terminal {
            match confirm_send(terminal, drafts_idx.is_some()) {
                Ok(a) => action = a,
                Err(e) => self.status_error = Some(e.to_string()),
            }
        }

        // ESC: return to editor without closing
        if action == SendAction::GoBack {
            return;
        }

        self.close_current_tab();

        match action {
            SendAction::Sent => {
                let draft = compose::parse_draft(&text);
                let mut addrs = draft.to.clone();
                addrs.extend(draft.cc.clone());
                self.address_book.harvest(&addrs);
                if !draft.to.is_empty()
                    && let Err(e) = send_message(
                        &self.config.smtp,
                        &draft.to,
                        &draft.cc,
                        &draft.subject,
                        &draft.body,
                        draft.in_reply_to.as_deref(),
                    )
                {
                    self.status_error = Some(e.to_string());
                }
            }
            SendAction::SaveDraft => {
                if let Some(idx) = drafts_idx {
                    let from = Address::new(
                        self.config.smtp.name.as_deref().unwrap_or(""),
                        &self.config.smtp.username,
                    );
                    let content = compose::draft_to_rfc2822(&text, &from.full());
                    if let Some(md) = self.maildirs.get_mut(idx) {
                        if let Err(e) = md.write_email(&content) {
                            self.status_error = Some(e.to_string());
                        } else {
                            md.sync();
                            if let Some(tv) = self.threads.get_mut(idx) {
                                tv.invalidate();
                            }
                        }
                    }
                }
            }
            SendAction::Discard | SendAction::GoBack => {}
        }
    }

    fn handle_email_tab_key(&mut self, key: KeyEvent, ei: usize) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('q')) => self.close_current_tab(),
            (_, KeyCode::Char('J')) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.next_email(1);
                }
                self.open_selected_email();
            }
            (_, KeyCode::Char('K')) => {
                if let Some(tv) = self.threads.get_mut(self.current_mb) {
                    tv.prev_email(1);
                }
                self.open_selected_email();
            }
            (_, KeyCode::Char('j') | KeyCode::Down) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.scroll_down(1);
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) | (_, KeyCode::PageDown) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.scroll_down(15);
                }
            }
            (_, KeyCode::Char('k') | KeyCode::Up) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.scroll_up(1);
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) | (_, KeyCode::PageUp) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.scroll_up(15);
                }
            }
            (_, KeyCode::Char('g')) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.first_line();
                }
            }
            (_, KeyCode::Char('G')) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    ev.last_line();
                }
            }
            (_, KeyCode::Char('m')) => {
                if self.config.mailboxes.len() > 1 {
                    self.move_mode = MoveMode::Active { selected: 0 };
                }
            }
            (_, KeyCode::Char('D')) => self.delete_current_tab_email(),
            (_, KeyCode::Char('r')) => self.open_reply_from_tab(false),
            (_, KeyCode::Char('R')) => self.open_reply_from_tab(true),
            (_, KeyCode::Char('Y')) => {
                if let Some(Tab::Email(ev)) = self.tabs.get_mut(ei) {
                    let raw = ev.raw_body();
                    if let Some(ref mut cb) = self.clipboard
                        && let Err(e) = cb.set_text(raw)
                    {
                        self.status_error = Some(e.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    fn reset_search(&mut self) {
        self.search = SearchMode::Off;
        if let Some(tv) = self.threads.get_mut(self.current_mb) {
            tv.set_search(None);
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let SearchMode::Typing(ref mut input) = self.search else {
            return;
        };
        match key.code {
            KeyCode::Char(c) => input.push(c),
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Enter => {
                let query = std::mem::take(input);
                self.search = if query.is_empty() {
                    SearchMode::Off
                } else {
                    SearchMode::Applied
                };
                return;
            }
            KeyCode::Esc => {
                self.reset_search();
                return;
            }
            _ => return,
        }
        // Live filter as user types
        let SearchMode::Typing(ref input) = self.search else {
            return;
        };
        if let Some(tv) = self.threads.get_mut(self.current_mb) {
            if input.is_empty() {
                tv.set_search(None);
            } else {
                tv.set_search(Some(input));
            }
        }
    }

    fn toggle_read(&mut self) {
        let Some(thread) = self
            .threads
            .get(self.current_mb)
            .and_then(|tv| tv.selected())
        else {
            return;
        };
        if thread.parent.is_unread() {
            thread.parent.mark_as_read();
        } else {
            thread.parent.mark_as_unread();
        }
    }

    fn delete_selected_thread(&mut self) {
        let Some(thread) = self
            .threads
            .get(self.current_mb)
            .and_then(|tv| tv.selected())
        else {
            return;
        };
        let id = thread.parent.message_id.clone();
        self.maildirs[self.current_mb].remove_by_id(&id);
        self.threads[self.current_mb].invalidate();
    }

    fn delete_current_tab_email(&mut self) {
        let ei = self.current_tab.saturating_sub(1);
        let Some(Tab::Email(ev)) = self.tabs.get(ei) else {
            return;
        };
        let id = ev.message_id().to_string();
        for (maildir, tv) in self.maildirs.iter_mut().zip(self.threads.iter_mut()) {
            maildir.remove_by_id(&id);
            tv.invalidate();
        }
        self.close_current_tab();
    }

    fn trigger_sync(&mut self) {
        if self.pending_sync.is_some() {
            return;
        }

        if let Some(sync_cfg) = &self.config.sync {
            let (tx, rx) = mpsc::channel();
            let cmd = sync_cfg.command.clone();

            self.pending_sync = Some(rx);

            std::thread::spawn(move || {
                let result = match std::process::Command::new("sh").args(["-c", &cmd]).output() {
                    Ok(output) if output.status.success() => None,
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let code = output.status.code().unwrap_or(-1);
                        Some(if stderr.is_empty() {
                            format!("exit {}", code)
                        } else {
                            stderr.trim().to_string()
                        })
                    }
                    Err(e) => Some(e.to_string()),
                };
                let _ = tx.send(result);
            });
        }
    }

    fn tab_count(&self) -> usize {
        1 + self.tabs.len()
    }

    fn open_selected_email(&mut self) {
        let thread = self
            .threads
            .get(self.current_mb)
            .and_then(|tv| tv.selected());
        let Some(thread) = thread else { return };
        let is_drafts = self
            .config
            .mailboxes
            .get(self.current_mb)
            .is_some_and(|mb| mb.is_drafts());
        if is_drafts {
            match compose::email_to_draft(&thread.parent) {
                Ok(draft) => self.open_editor(draft),
                Err(e) => self.status_error = Some(e.to_string()),
            }
        } else {
            thread.parent.mark_as_read();
            self.open_email(&thread.parent);
        }
    }

    fn open_email(&mut self, email: &Email) {
        if self.current_tab == 0 {
            // Main view: switch to existing tab or append a new one.
            if let Some(i) = self
                .tabs
                .iter()
                .position(|t| matches!(t, Tab::Email(ev) if ev.message_id() == email.message_id))
            {
                self.current_tab = i + 1;
                return;
            }
            if let Ok(ev) = EmailView::new(email) {
                self.tabs.push(Tab::Email(Box::new(ev)));
                self.current_tab = self.tabs.len();
            }
        } else {
            // Email tab: replace the current tab in place.
            let ei = self.current_tab.saturating_sub(1);
            if let Ok(ev) = EmailView::new(email)
                && let Some(slot) = self.tabs.get_mut(ei)
            {
                *slot = Tab::Email(Box::new(ev));
            }
        }
    }

    fn close_current_tab(&mut self) {
        if self.current_tab == 0 {
            return; // Brew tab cannot be closed
        }
        self.tabs.remove(self.current_tab.saturating_sub(1));
        if self.current_tab >= self.tab_count() {
            self.current_tab = self.current_tab.saturating_sub(1);
        }
        // else: current_tab now points to the next tab
    }

    fn compose(&mut self) {
        self.open_editor(compose::compose_draft());
    }

    fn open_reply_from_thread(&mut self, quote: bool) {
        let Some(thread) = self
            .threads
            .get(self.current_mb)
            .and_then(|tv| tv.selected())
        else {
            return;
        };
        match thread.parent.reply_draft(quote, &self.config.smtp.username) {
            Ok(draft) => self.open_editor(draft),
            Err(e) => self.status_error = Some(e.to_string()),
        }
    }

    fn open_reply_from_tab(&mut self, quote: bool) {
        let ei = self.current_tab.saturating_sub(1);
        let Some(Tab::Email(ev)) = self.tabs.get(ei) else {
            return;
        };
        let path = ev.path().to_path_buf();
        match Email::from_file(&path) {
            Ok(email) => match email.reply_draft(quote, &self.config.smtp.username) {
                Ok(draft) => self.open_editor(draft),
                Err(e) => self.status_error = Some(e.to_string()),
            },
            Err(e) => self.status_error = Some(e.to_string()),
        }
    }

    fn open_editor(&mut self, draft: String) {
        self.tabs.push(Tab::Compose(Box::new(Editor::new(&draft))));
        self.current_tab = self.tabs.len();
    }

    fn next_tab(&mut self) {
        self.current_tab = self.current_tab.saturating_add(1) % self.tab_count();
    }

    fn prev_tab(&mut self) {
        let total = self.tab_count();
        self.current_tab = (self.current_tab + total.saturating_sub(1)) % total;
    }

    fn next_mailbox(&mut self) {
        self.current_mb = self
            .current_mb
            .saturating_add(1)
            .min(self.maildirs.len().saturating_sub(1));
    }

    fn prev_mailbox(&mut self) {
        self.current_mb = self.current_mb.saturating_sub(1);
    }

    fn handle_move_key(&mut self, key: KeyEvent) {
        let targets_count = self.config.mailboxes.len().saturating_sub(1);
        let MoveMode::Active { ref mut selected } = self.move_mode else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                *selected = (*selected + 1).min(targets_count.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                *selected = selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                let sel = *selected;
                self.move_mode = MoveMode::Off;
                let target_idx = self
                    .config
                    .mailboxes
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.current_mb)
                    .nth(sel)
                    .map(|(i, _)| i);
                if let Some(idx) = target_idx {
                    self.move_selected_email(idx);
                }
            }
            KeyCode::Esc => {
                self.move_mode = MoveMode::Off;
            }
            _ => {}
        }
    }

    fn move_selected_email(&mut self, target_mb_idx: usize) {
        let Some(thread) = self
            .threads
            .get(self.current_mb)
            .and_then(|tv| tv.selected())
        else {
            return;
        };
        let path = thread.parent.path().clone();
        let message_id = thread.parent.message_id.clone();

        let target_dir = self.maildirs[target_mb_idx].path().to_string();
        let target_cur = std::path::Path::new(&target_dir).join("cur");
        let Some(filename) = path.file_name() else {
            return;
        };
        let dest = target_cur.join(filename);

        if let Err(e) = std::fs::create_dir_all(&target_cur)
            .and_then(|_| std::fs::copy(&path, &dest).map(|_| ()))
        {
            self.status_error = Some(e.to_string());
            return;
        }

        self.maildirs[self.current_mb].remove_by_id(&message_id);
        self.threads[self.current_mb].invalidate();

        self.maildirs[target_mb_idx].sync();
        self.threads[target_mb_idx].invalidate();
    }
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
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
            Tab::Compose(ed) => {
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
            Tab::Compose(ed) => editor::draw(frame, chunks[2], ed),
        }
    }

    draw_statusbar(frame, chunks[3], app);
    draw_move_popup(frame, app);
}

fn draw_main(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
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

fn draw_sidebar(
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

fn draw_list_popup(frame: &mut ratatui::Frame, title: &str, items: &[String], selected: usize) {
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

#[derive(PartialEq)]
enum SendAction {
    Sent,
    Discard,
    SaveDraft,
    GoBack,
}

fn confirm_send(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    has_drafts: bool,
) -> anyhow::Result<SendAction> {
    use crossterm::event::{Event, KeyCode};

    let labels: &[&str] = if has_drafts {
        &["Send Email", "Save as draft", "Discard"]
    } else {
        &["Send Email", "Discard"]
    };

    let to_action = |idx: usize| match labels[idx] {
        "Send Email" => SendAction::Sent,
        "Save as draft" => SendAction::SaveDraft,
        _ => SendAction::Discard,
    };

    let owned: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    let mut selected: usize = 0;

    loop {
        let sel = selected;
        terminal.draw(|frame| {
            draw_list_popup(frame, " Send message? ", &owned, sel);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(labels.len().saturating_sub(1));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Enter => {
                    return Ok(to_action(selected));
                }
                KeyCode::Esc => {
                    return Ok(SendAction::GoBack);
                }
                _ => {}
            }
        }
    }
}

fn send_message(
    smtp: &Smtp,
    to: &[Address],
    cc: &[Address],
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> anyhow::Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let from = Address::new(smtp.name.as_deref().unwrap_or(""), &smtp.username);
    let mut builder = Message::builder().from(
        from.full()
            .parse()
            .map_err(|e| anyhow!("invalid From address: {e}"))?,
    );
    for addr in to {
        builder = builder.to(addr
            .full()
            .parse()
            .map_err(|e| anyhow!("invalid To address {addr}: {e}"))?);
    }
    for addr in cc {
        builder = builder.cc(addr
            .full()
            .parse()
            .map_err(|e| anyhow!("invalid Cc address {addr}: {e}"))?);
    }
    if let Some(irt) = in_reply_to {
        builder = builder.in_reply_to(irt.to_string());
    }
    let email = builder
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| anyhow!("failed to build message: {e}"))?;

    let creds = Credentials::new(smtp.username.clone(), smtp.password.clone());
    let transport = SmtpTransport::starttls_relay(&smtp.host)
        .map_err(|e| anyhow!("SMTP relay error: {e}"))?
        .port(smtp.port)
        .credentials(creds)
        .build();

    transport
        .send(&email)
        .map_err(|e| anyhow!("SMTP send error: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn mb(label: &str, path: &str) -> config::Mailbox {
        config::Mailbox {
            label: label.to_string(),
            path: path.to_string(),
        }
    }

    fn make_config(mailboxes: Vec<config::Mailbox>) -> Config {
        Config {
            mailboxes,
            smtp: config::Smtp {
                host: String::new(),
                port: 0,
                username: String::new(),
                name: None,
                password: String::new(),
            },
            sync: None,
        }
    }

    fn make_app(mailboxes: Vec<config::Mailbox>) -> App {
        let cfg = make_config(mailboxes);
        let maildirs: Vec<Maildir> = cfg.mailboxes.iter().map(|_| Maildir::default()).collect();
        let threads: Vec<ThreadsView> = maildirs
            .iter()
            .map(|c| ThreadsView::new(c.threads()))
            .collect();
        let mut sidebar_state = ListState::default();
        if !cfg.mailboxes.is_empty() {
            sidebar_state.select(Some(0));
        }
        App {
            sidebar_state,
            config: cfg,
            maildirs,
            threads,
            tabs: Vec::new(),
            current_tab: 0,
            current_mb: 0,
            pending_sync: None,
            search: SearchMode::Off,
            move_mode: MoveMode::Off,
            status_error: None,
            terminal: None,
            address_book: AddressBook::load(),
            clipboard: Clipboard::new().ok(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn push_tab(app: &mut App, subject: &str) {
        app.tabs
            .push(Tab::Email(Box::new(EmailView::new_stub(subject))));
        app.current_tab = app.tabs.len();
    }

    fn make_threads(ids: &[&str]) -> crate::core::thread::EmailThreadList {
        use crate::core::thread::{Email, EmailThread, EmailThreadList};
        use std::cell::RefCell;
        use std::path::PathBuf;
        use std::rc::Rc;
        EmailThreadList::new(RefCell::new(
            ids.iter()
                .map(|id| {
                    Rc::new(EmailThread {
                        parent: Email::new(
                            id,
                            None,
                            "",
                            "",
                            None,
                            PathBuf::from(format!("/mb/new/{}", id)),
                        ),
                        replies: RefCell::new(Vec::new()),
                    })
                })
                .collect(),
        ))
    }

    // ── main view ────────────────────────────────────────────────────────────

    #[test]
    fn capital_q_in_main_quits() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        assert!(!app.handle_key(key(KeyCode::Char('Q'))));
    }

    #[test]
    fn lowercase_q_in_main_does_not_quit() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        assert!(app.handle_key(key(KeyCode::Char('q'))));
    }

    #[test]
    fn capital_j_moves_current_mb_down() {
        let mut app = make_app(vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")]);
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.config.mailboxes[app.current_mb].label, "Sent");
        assert_eq!(app.current_mb, 1);
    }

    #[test]
    fn capital_k_moves_current_mb_up() {
        let mut app = make_app(vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")]);
        app.handle_key(key(KeyCode::Char('J')));
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.config.mailboxes[app.current_mb].label, "Inbox");
        assert_eq!(app.current_mb, 0);
    }

    #[test]
    fn capital_j_clamped_at_last_mailbox() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.handle_key(key(KeyCode::Char('J')));
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.current_mb, 0);
    }

    #[test]
    fn capital_k_clamped_at_first_mailbox() {
        let mut app = make_app(vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")]);
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.current_mb, 0);
    }

    #[test]
    fn j_moves_thread_down() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn k_moves_thread_up() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn down_moves_thread_down() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn up_moves_thread_up() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "a");
    }

    // ── g/G first/last email ────────────────────────────────────────────────

    #[test]
    fn g_moves_to_first_email() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b", "c"]));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('j'))); // select "c"
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn g_on_empty_threads_does_not_panic() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&[]));
        app.handle_key(key(KeyCode::Char('g')));
        assert!(app.threads[0].selected().is_none());
    }

    #[test]
    fn capital_g_moves_to_last_email() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b", "c"]));
        app.handle_key(key(KeyCode::Char('G')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "c");
    }

    #[test]
    fn capital_g_on_empty_threads_does_not_panic() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&[]));
        app.handle_key(key(KeyCode::Char('G')));
        assert!(app.threads[0].selected().is_none());
    }

    // ── J/K in email tab ─────────────────────────────────────────────────────

    #[test]
    fn capital_j_in_email_tab_moves_thread_down() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        push_tab(&mut app, "current");
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn capital_k_in_email_tab_moves_thread_up() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.threads[0] = ThreadsView::new(make_threads(&["a", "b"]));
        // start on "b"
        if let Some(tv) = app.threads.get_mut(0) {
            tv.next_email(1);
        }
        push_tab(&mut app, "current");
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.threads[0].selected().unwrap().parent.message_id, "a");
    }

    // ── open_selected_email ──────────────────────────────────────────────────

    #[test]
    fn enter_on_already_open_email_switches_to_existing_tab() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);

        // Push a stub tab whose message_id is "stub@test"
        push_tab(&mut app, "Subject");
        let initial_tab_count = app.tabs.len();

        // Set up a thread with the same message_id as the stub
        app.threads[0] = ThreadsView::new(make_threads(&["stub@test"]));
        app.current_tab = 0; // back to Brew

        app.handle_key(key(KeyCode::Enter));

        // No new tab should have been opened
        assert_eq!(app.tabs.len(), initial_tab_count);
        // current_tab should switch to the existing tab (index 1)
        assert_eq!(app.current_tab, 1);
    }

    // ── tab view ─────────────────────────────────────────────────────────────

    #[test]
    fn brew_tab_cannot_be_closed() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        assert_eq!(app.current_tab, 0);
        app.close_current_tab();
        assert_eq!(app.current_tab, 0);
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn close_only_tab_returns_to_brew() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.tabs.is_empty());
        assert_eq!(app.current_tab, 0);
    }

    #[test]
    fn close_middle_tab_moves_to_next() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        push_tab(&mut app, "B");
        push_tab(&mut app, "C");
        app.current_tab = 2; // on B
        app.close_current_tab();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.current_tab, 2); // now pointing at C
    }

    #[test]
    fn close_last_tab_moves_to_previous() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        push_tab(&mut app, "B");
        app.current_tab = 2; // on B (last)
        app.close_current_tab();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.current_tab, 1); // moved to A
    }

    #[test]
    fn q_in_email_tab_returns_true() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "Subject A");
        assert!(app.handle_key(key(KeyCode::Char('q'))));
    }

    #[test]
    fn ctrl_n_cycles_tabs_from_any_view() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        app.current_tab = 0; // on Brew
        app.handle_key(ctrl(KeyCode::Char('n')));
        assert_eq!(app.current_tab, 1); // moved to email tab
        app.handle_key(ctrl(KeyCode::Char('n')));
        assert_eq!(app.current_tab, 0); // wrapped back to Brew
    }

    #[test]
    fn ctrl_n_from_brew_moves_to_first_email_tab() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        app.current_tab = 0;
        app.handle_key(ctrl(KeyCode::Char('n')));
        assert_eq!(app.current_tab, 1);
    }

    #[test]
    fn ctrl_n_wraps_from_last_tab_to_brew() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        push_tab(&mut app, "B");
        app.current_tab = 2;
        app.handle_key(ctrl(KeyCode::Char('n')));
        assert_eq!(app.current_tab, 0);
    }

    #[test]
    fn ctrl_p_from_brew_wraps_to_last_email_tab() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        push_tab(&mut app, "B");
        app.current_tab = 0;
        app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(app.current_tab, 2);
    }

    #[test]
    fn ctrl_p_moves_to_prev_tab() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        push_tab(&mut app, "B");
        app.current_tab = 2;
        app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(app.current_tab, 1);
    }

    // ── draw ─────────────────────────────────────────────────────────────────

    #[test]
    fn draw_brew_tab_always_present() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let first_row: String = (0..buf.area().width)
            .map(|x| buf.cell((x, 0)).map_or(" ", |c| c.symbol()))
            .collect();
        assert!(first_row.contains("Brew"), "got: {first_row}");
    }

    #[test]
    fn draw_email_tab_subject_in_tab_bar() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "Hello subject");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let first_row: String = (0..buf.area().width)
            .map(|x| buf.cell((x, 0)).map_or(" ", |c| c.symbol()))
            .collect();
        assert!(first_row.contains("Hello subject"), "got: {first_row}");
    }

    // ── delete ────────────────────────────────────────────────────────────────

    #[test]
    fn capital_d_in_main_with_no_threads_is_noop() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        assert!(app.handle_key(key(KeyCode::Char('D'))));
    }

    #[test]
    fn capital_d_in_email_tab_closes_tab() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        assert_eq!(app.tabs.len(), 1);
        app.handle_key(key(KeyCode::Char('D')));
        assert!(app.tabs.is_empty());
        assert_eq!(app.current_tab, 0);
    }

    #[test]
    fn capital_d_in_email_tab_returns_true() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        push_tab(&mut app, "A");
        assert!(app.handle_key(key(KeyCode::Char('D'))));
    }

    // ── sidebar ──────────────────────────────────────────────────────────────

    fn rendered_sidebar_lines(
        state: &mut ListState,
        mailboxes: &[config::Mailbox],
        maildirs: &[Maildir],
        unread_filters: &[bool],
        w: u16,
        h: u16,
    ) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_sidebar(
                    frame,
                    ratatui::layout::Rect::new(0, 0, w, h),
                    state,
                    mailboxes,
                    maildirs,
                    unread_filters,
                );
            })
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

    #[test]
    fn new_empty_mailboxes_has_no_sidebar_selection() {
        let app = make_app(vec![]);
        assert!(app.sidebar_state.selected().is_none());
    }

    #[test]
    fn new_nonempty_mailboxes_selects_first_in_sidebar() {
        let app = make_app(vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")]);
        assert_eq!(app.sidebar_state.selected(), Some(0));
        assert_eq!(app.current_mb, 0);
    }

    #[test]
    fn draw_sidebar_shows_mailbox_title() {
        let mut state = ListState::default();
        state.select(Some(0));
        let mailboxes = vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")];
        let lines = rendered_sidebar_lines(&mut state, &mailboxes, &[], &[], 30, 6);
        let content: String = lines.join("\n");
        assert!(content.contains("Inbox"), "got:\n{}", content);
    }

    #[test]
    fn draw_sidebar_renders_all_labels() {
        let mut state = ListState::default();
        state.select(Some(0));
        let mailboxes = vec![
            mb("Inbox", "/inbox"),
            mb("Sent", "/sent"),
            mb("Drafts", "/drafts"),
        ];
        let lines = rendered_sidebar_lines(&mut state, &mailboxes, &[], &[], 30, 6);
        let content: String = lines.join("\n");
        assert!(content.contains("Inbox"), "got:\n{}", content);
        assert!(content.contains("Sent"), "got:\n{}", content);
        assert!(content.contains("Drafts"), "got:\n{}", content);
    }

    #[test]
    fn draw_sidebar_shows_unread_count() {
        use crate::core::maildir::Maildir;

        let mut state = ListState::default();
        state.select(Some(0));
        let mailboxes = vec![mb("Inbox", "/mail/inbox")];
        // Maildir with unread emails will show count
        let maildirs = vec![Maildir::default()];
        let lines = rendered_sidebar_lines(&mut state, &mailboxes, &maildirs, &[], 30, 6);
        let content: String = lines.join("\n");
        // Default Maildir has no unread, so just label shown
        assert!(content.contains("Inbox"), "got:\n{}", content);
    }

    #[test]
    fn sidebar_state_syncs_with_current_mb_on_draw() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = make_app(vec![mb("Inbox", "/inbox"), mb("Sent", "/sent")]);
        app.current_mb = 1;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_main(frame, frame.area(), &mut app))
            .unwrap();
        // After draw_main, sidebar_state should be synced with current_mb
        assert_eq!(app.sidebar_state.selected(), Some(1));
    }

    // ── search ───────────────────────────────────────────────────────────────

    #[test]
    fn slash_enters_search_mode() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.handle_key(key(KeyCode::Char('/')));
        assert!(matches!(app.search, SearchMode::Typing(_)));
    }

    #[test]
    fn search_esc_clears_and_exits() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('i')));
        assert!(matches!(app.search, SearchMode::Typing(ref s) if s == "hi"));
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.search, SearchMode::Off));
        assert!(app.threads[0].search().is_none());
    }

    #[test]
    fn search_enter_keeps_filter() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('t')));
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.search, SearchMode::Applied));
        assert_eq!(app.threads[0].search(), Some("t"));
    }

    #[test]
    fn search_esc_from_applied_clears_filter() {
        let mut app = make_app(vec![mb("Inbox", "/inbox")]);
        app.handle_key(key(KeyCode::Char('/')));
        app.handle_key(key(KeyCode::Char('x')));
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.search, SearchMode::Applied));
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.search, SearchMode::Off));
        assert!(app.threads[0].search().is_none());
    }
}
