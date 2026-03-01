// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::cache::EmailThread;
use crate::core::cache::MailCache;
use crate::core::config::{Mailbox, Smtp, load_signature, load_thanks};
use crate::core::read::mark_seen;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use notify::{EventKind, RecursiveMode, Watcher, recommended_watcher};
use ratatui::widgets::ListState;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::draw::draw;
use super::mail::{delete_mail, reply, thanks_reply};
use super::tab::EmailTab;

// ── flat list entry ───────────────────────────────────────────────────────────

pub struct Entry {
    pub depth: usize,
    pub thread: Rc<EmailThread>,
}

pub fn flatten(threads: &[Rc<EmailThread>], depth: usize, out: &mut Vec<Entry>) {
    for thread in threads {
        out.push(Entry {
            depth,
            thread: thread.clone(),
        });
        let replies = thread.replies.borrow();
        flatten(&replies, depth + 1, out);
    }
}

// ── app state ─────────────────────────────────────────────────────────────────

pub struct App {
    /// Which top-level tab is shown: 0 = list view, 1+ = open email tabs.
    pub active: usize,
    pub emails: Vec<EmailTab>,
    /// Index of the currently highlighted mailbox in the left pane.
    pub selected_mailbox: usize,
    pub mailbox_list_state: ListState,
    /// Per-mailbox thread list state (selection within threads).
    pub thread_list_states: Vec<ListState>,
    /// Tracks the current on-disk path for emails that have been marked seen
    /// (keyed by Message-ID). The cache holds the original pre-rename path;
    /// this map overrides it so re-opening an already-read email works.
    pub seen_paths: HashMap<String, PathBuf>,
}

impl App {
    pub fn new(mailbox_count: usize) -> Self {
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
            seen_paths: HashMap::new(),
        }
    }

    pub fn tab_count(&self) -> usize {
        1 + self.emails.len()
    }

    pub fn go_left(&mut self) {
        if self.active > 0 {
            self.active -= 1;
        }
    }

    pub fn go_right(&mut self) {
        if self.active + 1 < self.tab_count() {
            self.active += 1;
        }
    }

    pub fn go_next(&mut self) {
        self.active = (self.active + 1) % self.tab_count();
    }

    pub fn go_prev(&mut self) {
        let n = self.tab_count();
        self.active = (self.active + n - 1) % n;
    }

    pub fn close_active(&mut self) {
        if self.active == 0 {
            return;
        }
        self.emails.remove(self.active - 1);
        self.active = self.active.min(self.tab_count() - 1);
    }

    /// The currently selected thread index within the active mailbox.
    pub fn selected_thread(&self) -> Option<usize> {
        self.thread_list_states[self.selected_mailbox].selected()
    }

    /// Move thread selection down within the active mailbox.
    pub fn thread_down(&mut self, mailbox_entries: &[Vec<Entry>]) {
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
    pub fn thread_up(&mut self) {
        let cur = self.thread_list_states[self.selected_mailbox]
            .selected()
            .unwrap_or(0);
        self.thread_list_states[self.selected_mailbox].select(Some(cur.saturating_sub(1)));
    }

    /// Move thread selection to first thread.
    pub fn thread_home(&mut self) {
        self.thread_list_states[self.selected_mailbox].select(Some(0));
    }

    /// Jump thread selection by `delta` rows (positive = down, negative = up),
    /// clamped to the valid range.
    pub fn thread_skip(&mut self, mailbox_entries: &[Vec<Entry>], delta: isize) {
        let len = mailbox_entries[self.selected_mailbox].len();
        if len == 0 {
            return;
        }
        let cur = self.thread_list_states[self.selected_mailbox]
            .selected()
            .unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, len as isize - 1) as usize;
        self.thread_list_states[self.selected_mailbox].select(Some(next));
    }

    /// Move thread selection to last thread.
    pub fn thread_end(&mut self, mailbox_entries: &[Vec<Entry>]) {
        let len = mailbox_entries[self.selected_mailbox].len();
        if len > 0 {
            self.thread_list_states[self.selected_mailbox].select(Some(len - 1));
        }
    }

    /// Switch to the next mailbox.
    pub fn mailbox_down(&mut self, count: usize) {
        if self.selected_mailbox + 1 < count {
            self.selected_mailbox += 1;
            self.mailbox_list_state.select(Some(self.selected_mailbox));
        }
    }

    /// Switch to the previous mailbox.
    pub fn mailbox_up(&mut self) {
        if self.selected_mailbox > 0 {
            self.selected_mailbox -= 1;
            self.mailbox_list_state.select(Some(self.selected_mailbox));
        }
    }
}

// ── event loop ────────────────────────────────────────────────────────────────

pub fn run(mailbox_cfgs: &[&Mailbox], smtp: &Smtp) -> Result<()> {
    let signature = load_signature();
    let thanks = load_thanks();

    let new_mail = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&new_mail);
    let mut watcher = recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_))
        {
            flag.store(true, Ordering::Relaxed);
        }
    })?;
    for mb in mailbox_cfgs {
        let path = std::path::Path::new(&mb.path);
        if path.exists() {
            let _ = watcher.watch(path, RecursiveMode::Recursive);
        }
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        mailbox_cfgs,
        signature.as_deref(),
        thanks.as_deref(),
        smtp,
        new_mail,
    );

    drop(watcher);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mailbox_cfgs: &[&Mailbox],
    signature: Option<&str>,
    thanks: Option<&str>,
    smtp: &Smtp,
    new_mail: Arc<AtomicBool>,
) -> Result<()> {
    let labels: Vec<&str> = mailbox_cfgs.iter().map(|mb| mb.label.as_str()).collect();

    let build_entries = |cfgs: &[&Mailbox]| -> Vec<Vec<Entry>> {
        cfgs.iter()
            .map(|mb| {
                let mut v = Vec::new();
                if let Ok(cache) = MailCache::build(&mb.path) {
                    flatten(&cache.threads, 0, &mut v);
                }
                v
            })
            .collect()
    };

    let mut mailbox_entries = build_entries(mailbox_cfgs);
    let mut app = App::new(labels.len());

    loop {
        if new_mail.swap(false, Ordering::Relaxed) {
            mailbox_entries = build_entries(mailbox_cfgs);
            for (i, state) in app.thread_list_states.iter_mut().enumerate() {
                let len = mailbox_entries[i].len();
                if let Some(sel) = state.selected()
                    && sel >= len
                    && len > 0
                {
                    state.select(Some(len - 1));
                }
            }
        }

        terminal.draw(|frame| draw(frame, &mut app, &labels, &mailbox_entries))?;

        if !event::poll(std::time::Duration::from_secs(1))? {
            continue;
        }

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
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.go_next();
                    continue;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.go_prev();
                    continue;
                }
                _ => {}
            }

            if app.active == 0 {
                // ── list view ──
                let entries = &mailbox_entries[app.selected_mailbox];
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => app.thread_down(&mailbox_entries),
                    KeyCode::Char('k') | KeyCode::Up => app.thread_up(),
                    KeyCode::Home => app.thread_home(),
                    KeyCode::End => app.thread_end(&mailbox_entries),
                    KeyCode::PageDown => app.thread_skip(&mailbox_entries, 15),
                    KeyCode::PageUp => app.thread_skip(&mailbox_entries, -15),
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.thread_skip(&mailbox_entries, 15);
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.thread_skip(&mailbox_entries, -15);
                    }
                    KeyCode::Char('J') => app.mailbox_down(labels.len()),
                    KeyCode::Char('K') => app.mailbox_up(),
                    KeyCode::Enter => {
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                tab.path = new_path;
                                app.emails.push(tab);
                                app.active = app.tab_count() - 1;
                            }
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                tab.path = new_path;
                                let _ = reply(&tab, true, signature, smtp, terminal);
                            }
                        }
                    }
                    KeyCode::Char('R') => {
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                tab.path = new_path;
                                let _ = reply(&tab, false, signature, smtp, terminal);
                            }
                        }
                    }
                    KeyCode::Char('t') => {
                        if let (Some(thanks_body), Some(ti)) = (thanks, app.selected_thread()) {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                tab.path = new_path;
                                let _ = thanks_reply(&tab, thanks_body, signature, smtp, terminal);
                            }
                        }
                    }
                    KeyCode::Char('D') => {
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            delete_mail(eff_path);
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
                    KeyCode::Char('D') => {
                        let path = app.emails[ei].path.clone();
                        app.close_active();
                        delete_mail(&path);
                    }
                    KeyCode::Char('r') => {
                        let _ = reply(&app.emails[ei], true, signature, smtp, terminal);
                    }
                    KeyCode::Char('R') => {
                        let _ = reply(&app.emails[ei], false, signature, smtp, terminal);
                    }
                    KeyCode::Char('t') => {
                        if let Some(thanks_body) = thanks {
                            let _ = thanks_reply(
                                &app.emails[ei],
                                thanks_body,
                                signature,
                                smtp,
                                terminal,
                            );
                        }
                    }
                    KeyCode::Char('J') => {
                        app.thread_down(&mailbox_entries);
                        let entries = &mailbox_entries[app.selected_mailbox];
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut new_tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&new_tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                new_tab.path = new_path;
                                app.emails[ei] = new_tab;
                            }
                        }
                    }
                    KeyCode::Char('K') => {
                        app.thread_up();
                        let entries = &mailbox_entries[app.selected_mailbox];
                        if let Some(ti) = app.selected_thread() {
                            let meta = &entries[ti].thread.data;
                            let eff_path = app
                                .seen_paths
                                .get(&meta.message_id)
                                .map(|p| p.as_path())
                                .unwrap_or(&meta.path);
                            if let Ok(mut new_tab) = EmailTab::from_meta_at(meta, eff_path) {
                                let new_path = mark_seen(&new_tab.path);
                                app.seen_paths
                                    .insert(meta.message_id.clone(), new_path.clone());
                                new_tab.path = new_path;
                                app.emails[ei] = new_tab;
                            }
                        }
                    }
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
