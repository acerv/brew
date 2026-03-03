// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::cache::EmailThread;
use crate::core::cache::MailCache;
use crate::core::config::{Mailbox, Smtp, Sync, load_signature, load_thanks};
use crate::core::read::{is_unread, mark_seen};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher, recommended_watcher};
use ratatui::widgets::ListState;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use super::draw::draw;
use super::mail::{compose_new, delete_mail, reply, thanks_reply};
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
    /// Per-mailbox filter flag: when `true` the thread list shows only unread
    /// emails. Toggled with `N` (enable) and `n` (disable).
    pub unread_only: Vec<bool>,
    /// The last sync error message, if any. `None` when the last sync
    /// succeeded or no sync has run yet.
    pub sync_error: Option<String>,
    /// Current subject search query. Empty string means no filter.
    pub search_query: String,
    /// When `true`, the user is actively typing a search query.
    pub search_active: bool,
}

impl App {
    pub fn new(mailbox_count: usize) -> Self {
        let mut mailbox_list_state = ListState::default();
        if mailbox_count > 0 {
            mailbox_list_state.select(Some(0));
        }
        let thread_list_states: Vec<ListState> = (0..mailbox_count)
            .map(|_| {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            })
            .collect();
        Self {
            active: 0,
            emails: Vec::new(),
            selected_mailbox: 0,
            mailbox_list_state,
            thread_list_states,
            seen_paths: HashMap::new(),
            unread_only: vec![false; mailbox_count],
            sync_error: None,
            search_query: String::new(),
            search_active: false,
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

    /// Resolve the effective on-disk path for the currently selected thread,
    /// accounting for any `mark_seen` rename stored in `seen_paths`.
    /// Returns `None` when there is no thread selected or the mailbox is empty.
    pub fn effective_path<'a>(&'a self, entries: &'a [Entry]) -> Option<&'a std::path::Path> {
        let ti = self.selected_thread()?;
        let meta = &entries.get(ti)?.thread.data;
        Some(
            self.seen_paths
                .get(&meta.message_id)
                .map(|p| p.as_path())
                .unwrap_or(&meta.path),
        )
    }

    /// Load and mark-seen the currently selected thread, returning the
    /// resulting [`EmailTab`] and its new on-disk path.
    /// Returns `None` when there is no selection or loading fails.
    pub fn resolve_selected(&mut self, entries: &[Entry]) -> Option<EmailTab> {
        let ti = self.selected_thread()?;
        let meta = &entries.get(ti)?.thread.data;
        let eff_path = self
            .seen_paths
            .get(&meta.message_id)
            .map(|p| p.as_path())
            .unwrap_or(&meta.path);
        let mut tab = EmailTab::from_meta_at(meta, eff_path).ok()?;
        let new_path = mark_seen(&tab.path);
        self.seen_paths
            .insert(meta.message_id.clone(), new_path.clone());
        tab.path = new_path;
        Some(tab)
    }
}

// ── filter helpers ────────────────────────────────────────────────────────────

fn filter_mailbox(
    entries: &[Entry],
    unread_only: bool,
    search_query: &str,
    seen: &HashMap<String, PathBuf>,
) -> Vec<Entry> {
    let q = search_query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            if unread_only {
                let eff = seen
                    .get(&e.thread.data.message_id)
                    .map(|p| p.as_path())
                    .unwrap_or(&e.thread.data.path);
                if !is_unread(eff) {
                    return false;
                }
            }
            if !q.is_empty() && !e.thread.data.subject.to_lowercase().contains(&q) {
                return false;
            }
            true
        })
        .map(|e| Entry {
            depth: e.depth,
            thread: e.thread.clone(),
        })
        .collect()
}

fn apply_all_filters(
    entries: &[Vec<Entry>],
    unread_only: &[bool],
    search_query: &str,
    seen: &HashMap<String, PathBuf>,
) -> Vec<Vec<Entry>> {
    entries
        .iter()
        .enumerate()
        .map(|(i, v)| filter_mailbox(v, unread_only[i], search_query, seen))
        .collect()
}

// ── event loop ────────────────────────────────────────────────────────────────

pub fn run(mailbox_cfgs: &[&Mailbox], smtp: &Smtp, sync_cfg: Option<&Sync>) -> Result<()> {
    let signature = load_signature();
    let thanks = load_thanks();

    // Canonicalize every mailbox path once so that prefix-matching against the
    // absolute paths that `notify` reports in events is always correct,
    // regardless of whether config uses `~`, relative paths, or symlinks.
    let canonical_paths: Vec<PathBuf> = mailbox_cfgs
        .iter()
        .map(|mb| std::fs::canonicalize(&mb.path).unwrap_or_else(|_| PathBuf::from(&mb.path)))
        .collect();

    let (tx, rx) = mpsc::channel::<notify::Event>();
    let mut watcher = recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && matches!(
                ev.kind,
                EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(ModifyKind::Name(_))
            )
        {
            let _ = tx.send(ev);
        }
    })?;
    for path in &canonical_paths {
        if path.exists() {
            let _ = watcher.watch(path, RecursiveMode::Recursive);
        }
    }

    // Spawn the background sync thread when a [sync] section is configured.
    // Dropping `shutdown_tx` (at the end of this function) signals the thread
    // to stop after its current sleep interval.
    let (sync_tx, sync_rx) = mpsc::channel::<Option<String>>();
    let _shutdown_tx = sync_cfg.map(|s| {
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        crate::core::sync::spawn(s.command.clone(), s.interval, sync_tx, shutdown_rx);
        shutdown_tx
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        mailbox_cfgs,
        &canonical_paths,
        signature.as_deref(),
        thanks.as_deref(),
        smtp,
        rx,
        sync_rx,
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
    canonical_paths: &[PathBuf],
    signature: Option<&str>,
    thanks: Option<&str>,
    smtp: &Smtp,
    fs_events: mpsc::Receiver<notify::Event>,
    sync_rx: mpsc::Receiver<Option<String>>,
) -> Result<()> {
    let labels: Vec<&str> = mailbox_cfgs.iter().map(|mb| mb.label.as_str()).collect();

    // Build the initial per-mailbox caches using the canonical paths so that
    // the paths stored in EmailMeta match what notify will report in events.
    let mut caches: Vec<MailCache> = canonical_paths
        .iter()
        .map(|p| {
            p.to_str()
                .and_then(|s| MailCache::build(s).ok())
                .unwrap_or(MailCache { threads: vec![] })
        })
        .collect();

    let flatten_all = |caches: &[MailCache]| -> Vec<Vec<Entry>> {
        caches
            .iter()
            .map(|cache| {
                let mut v = Vec::new();
                flatten(&cache.threads, 0, &mut v);
                v
            })
            .collect()
    };

    let mut mailbox_entries = flatten_all(&caches);
    let mut app = App::new(labels.len());

    // `filtered_entries` is the view actually shown and navigated.
    // It equals `mailbox_entries` when unread_only[i] is false, and is
    // filtered to unread emails only when true.
    let mut filtered_entries = apply_all_filters(
        &mailbox_entries,
        &app.unread_only,
        &app.search_query,
        &app.seen_paths,
    );

    let tick = std::time::Duration::from_secs(60);
    let mut last_tick = std::time::Instant::now();

    loop {
        // Drain sync results: None = success (clear error), Some(msg) = failure.
        for msg in sync_rx.try_iter() {
            app.sync_error = msg;
        }

        // Drain all pending filesystem events and apply them incrementally.
        let mut changed: Vec<bool> = vec![false; mailbox_cfgs.len()];
        for ev in fs_events.try_iter() {
            // Classify the event into per-path actions.
            // Renames (tmp/ -> cur/, or new/ -> cur/ for mark_seen) appear as
            // Modify(Name(_)) events. The convention is: for Both, paths = [from, to];
            // for From/To, a single path in the respective role.
            // We treat the From/source path as a remove and the To/dest as a create,
            // but only when those paths land inside new/ or cur/.
            let actions: Vec<(&PathBuf, bool)> = match ev.kind {
                EventKind::Create(_) => ev.paths.iter().map(|p| (p, true)).collect(),
                EventKind::Remove(_) => ev.paths.iter().map(|p| (p, false)).collect(),
                EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Both)) => {
                    // paths = [from, to]
                    let mut v = Vec::new();
                    if let Some(from) = ev.paths.first() {
                        v.push((from, false));
                    }
                    if let Some(to) = ev.paths.last() {
                        v.push((to, true));
                    }
                    v
                }
                EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::From)) => {
                    ev.paths.iter().map(|p| (p, false)).collect()
                }
                EventKind::Modify(ModifyKind::Name(_)) => {
                    // To or Any — treat as create
                    ev.paths.iter().map(|p| (p, true)).collect()
                }
                _ => vec![],
            };

            for (path, is_create) in actions {
                // Only act on files that live directly inside new/ or cur/.
                let parent_name = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if parent_name != "new" && parent_name != "cur" {
                    continue;
                }
                if let Some(mb_idx) = canonical_paths.iter().position(|cp| path.starts_with(cp)) {
                    if is_create {
                        caches[mb_idx].insert_file(path);
                    } else {
                        caches[mb_idx].remove_file(path);
                    }
                    changed[mb_idx] = true;
                }
            }
        }

        // Re-flatten only the mailboxes that actually changed, then re-filter.
        for (i, did_change) in changed.iter().enumerate() {
            if *did_change {
                let mut v = Vec::new();
                flatten(&caches[i].threads, 0, &mut v);
                mailbox_entries[i] = v;
                // Re-apply the unread filter for this mailbox.
                filtered_entries[i] = filter_mailbox(
                    &mailbox_entries[i],
                    app.unread_only[i],
                    &app.search_query,
                    &app.seen_paths,
                );
                // Clamp selection if the visible list shrank.
                let len = filtered_entries[i].len();
                if let Some(sel) = app.thread_list_states[i].selected()
                    && sel >= len
                    && len > 0
                {
                    app.thread_list_states[i].select(Some(len - 1));
                } else if len == 0 {
                    app.thread_list_states[i].select(None);
                }
            }
        }

        terminal.draw(|frame| draw(frame, &mut app, &labels, &filtered_entries))?;

        let remaining = tick.saturating_sub(last_tick.elapsed());
        if !event::poll(remaining)? {
            last_tick = std::time::Instant::now();
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Tab navigation is disabled while typing a search query.
            if !(app.active == 0 && app.search_active) {
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
            }

            if app.active == 0 {
                let mb = app.selected_mailbox;
                if app.search_active {
                    // ── search input ──
                    match key.code {
                        KeyCode::Esc => {
                            app.search_active = false;
                            app.search_query.clear();
                            filtered_entries = apply_all_filters(
                                &mailbox_entries,
                                &app.unread_only,
                                &app.search_query,
                                &app.seen_paths,
                            );
                            let len = filtered_entries[mb].len();
                            app.thread_list_states[mb].select(if len > 0 { Some(0) } else { None });
                        }
                        KeyCode::Enter => {
                            app.search_active = false;
                            if app.search_query.is_empty() {
                                filtered_entries = apply_all_filters(
                                    &mailbox_entries,
                                    &app.unread_only,
                                    "",
                                    &app.seen_paths,
                                );
                                let len = filtered_entries[mb].len();
                                app.thread_list_states[mb].select(if len > 0 {
                                    Some(0)
                                } else {
                                    None
                                });
                            }
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                            filtered_entries = apply_all_filters(
                                &mailbox_entries,
                                &app.unread_only,
                                &app.search_query,
                                &app.seen_paths,
                            );
                            let len = filtered_entries[mb].len();
                            app.thread_list_states[mb].select(if len > 0 { Some(0) } else { None });
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                            filtered_entries = apply_all_filters(
                                &mailbox_entries,
                                &app.unread_only,
                                &app.search_query,
                                &app.seen_paths,
                            );
                            let len = filtered_entries[mb].len();
                            app.thread_list_states[mb].select(if len > 0 { Some(0) } else { None });
                        }
                        _ => {}
                    }
                } else {
                    // ── list view ──
                    let entries = &filtered_entries[mb];
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('/') => {
                            app.search_active = true;
                            app.search_query.clear();
                        }
                        KeyCode::Char('j') | KeyCode::Down => app.thread_down(&filtered_entries),
                        KeyCode::Char('k') | KeyCode::Up => app.thread_up(),
                        KeyCode::Char('g') => app.thread_home(),
                        KeyCode::Char('G') => app.thread_end(&filtered_entries),
                        KeyCode::PageDown => app.thread_skip(&filtered_entries, 15),
                        KeyCode::PageUp => app.thread_skip(&filtered_entries, -15),
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.thread_skip(&filtered_entries, 15);
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.thread_skip(&filtered_entries, -15);
                        }
                        KeyCode::Char('J') => app.mailbox_down(labels.len()),
                        KeyCode::Char('K') => app.mailbox_up(),
                        KeyCode::Enter => {
                            if let Some(tab) = app.resolve_selected(entries) {
                                app.emails.push(tab);
                                app.active = app.tab_count() - 1;
                            }
                        }
                        KeyCode::Char('r') => {
                            if let Some(tab) = app.resolve_selected(entries) {
                                let _ = reply(&tab, true, signature, smtp, terminal);
                            }
                        }
                        KeyCode::Char('R') => {
                            if let Some(tab) = app.resolve_selected(entries) {
                                let _ = reply(&tab, false, signature, smtp, terminal);
                            }
                        }
                        KeyCode::Char('t') => {
                            if let Some(thanks_body) = thanks
                                && let Some(tab) = app.resolve_selected(entries)
                            {
                                let _ = thanks_reply(&tab, thanks_body, signature, smtp, terminal);
                            }
                        }
                        KeyCode::Char('D') => {
                            if let Some(eff_path) = app.effective_path(entries) {
                                delete_mail(eff_path);
                            }
                        }
                        KeyCode::Char('N') => {
                            app.unread_only[mb] = true;
                            filtered_entries[mb] = filter_mailbox(
                                &mailbox_entries[mb],
                                true,
                                &app.search_query,
                                &app.seen_paths,
                            );
                            let len = filtered_entries[mb].len();
                            app.thread_list_states[mb].select(if len > 0 { Some(0) } else { None });
                        }
                        KeyCode::Char('n') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.unread_only[mb] = false;
                            filtered_entries[mb] = filter_mailbox(
                                &mailbox_entries[mb],
                                false,
                                &app.search_query,
                                &app.seen_paths,
                            );
                            let len = filtered_entries[mb].len();
                            app.thread_list_states[mb].select(if len > 0 { Some(0) } else { None });
                        }
                        KeyCode::Char('C') => {
                            let _ = compose_new(signature, smtp, terminal);
                        }
                        _ => {}
                    }
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
                        if let Some(new_tab) = app.resolve_selected(entries) {
                            app.emails[ei] = new_tab;
                        }
                    }
                    KeyCode::Char('K') => {
                        app.thread_up();
                        let entries = &mailbox_entries[app.selected_mailbox];
                        if let Some(new_tab) = app.resolve_selected(entries) {
                            app.emails[ei] = new_tab;
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
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
