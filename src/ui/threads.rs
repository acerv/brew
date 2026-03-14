// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::date::humanize_date;
use crate::core::thread::{EmailThread, EmailThreadList, Flag};
use crate::ui::utils;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};
use std::rc::Rc;

struct Row {
    depth: usize,
    thread: Rc<EmailThread>,
}

/// Scrollable thread list sharing the same `ThreadList` as the `Maildir`.
/// Any structural change made to the maildir (insert/remove/invalidate) is
/// reflected here after calling `invalidate()`.
pub struct ThreadsView {
    threads: EmailThreadList,
    state: ListState,
    rows: Vec<Row>,
    unread_only: bool,
    search: Option<String>,
}

impl ThreadsView {
    fn flatten(&mut self) {
        self.rows.clear();
        flatten_recursive(&self.threads.borrow(), 0, &mut self.rows);
    }

    /// Build the view from a shared `ThreadList`.
    pub fn new(threads: EmailThreadList) -> Self {
        let mut view = Self {
            threads,
            state: ListState::default(),
            rows: Vec::new(),
            unread_only: false,
            search: None,
        };
        view.flatten();
        if !view.rows.is_empty() {
            view.state.select(Some(0));
        }
        view
    }

    pub fn prev_email(&mut self, n: usize) {
        if self.rows.is_empty() {
            return;
        }

        if let Some(i) = self.state.selected() {
            self.state.select(Some(i.saturating_sub(n)));
        }
    }

    pub fn next_email(&mut self, n: usize) {
        if self.rows.is_empty() {
            return;
        }

        if let Some(i) = self.state.selected() {
            let next = i.saturating_add(n).min(self.rows.len().saturating_sub(1));
            self.state.select(Some(next));
        }
    }

    pub fn first_email(&mut self) {
        if self.rows.is_empty() {
            return;
        }

        self.state.select(Some(0));
    }

    pub fn last_email(&mut self) {
        if self.rows.is_empty() {
            return;
        }

        self.state.select(Some(self.rows.len().saturating_sub(1)));
    }

    /// Return the currently highlighted thread, if any.
    pub fn selected(&self) -> Option<Rc<EmailThread>> {
        self.state
            .selected()
            .and_then(|i| self.rows.get(i))
            .map(|r| r.thread.clone())
    }

    /// Re-flatten from the shared `ThreadList`, preserving the selection by
    /// message-id if still present, otherwise clamping to the new length.
    pub fn invalidate(&mut self) {
        let old_idx = self.state.selected().unwrap_or(0);
        let selected_id = self
            .rows
            .get(old_idx)
            .map(|r| r.thread.parent.message_id.clone());

        self.flatten();

        if self.unread_only {
            self.rows.retain(|r| r.thread.parent.is_unread());
        }

        if let Some(q) = &self.search {
            let q = q.clone();
            self.rows
                .retain(|r| r.thread.parent.subject.to_lowercase().contains(&q));
        }

        let new_idx = selected_id
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|r| r.thread.parent.message_id == id)
            })
            .unwrap_or_else(|| old_idx.min(self.rows.len().saturating_sub(1)));

        if self.rows.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(new_idx));
        }
    }

    /// Return whether the view is currently filtering to unread only.
    pub fn is_unread_only(&self) -> bool {
        self.unread_only
    }

    /// Toggle between unread-only and all emails.
    pub fn toggle_unread(&mut self) {
        self.unread_only = !self.unread_only;
        self.invalidate();
    }

    /// Set or clear the subject search filter and refresh.
    pub fn set_search(&mut self, query: Option<&str>) {
        self.search = query.map(|q| q.to_lowercase());
        self.invalidate();
    }

    /// Return the current search query, if any.
    pub fn search(&self) -> Option<&str> {
        self.search.as_deref()
    }
}

/// Render the thread list into `area`.
pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, view: &mut ThreadsView) {
    const FROM_W: usize = 27;
    const DATE_W: usize = 16;
    const FLAGS_W: usize = 4; // "★↩→ " etc.
    let usable = area.width.saturating_sub(2) as usize;
    let subject_w = usable.saturating_sub(FROM_W + DATE_W + FLAGS_W + 2);

    let items: Vec<ListItem> = view
        .rows
        .iter()
        .map(|row| {
            let e = &row.thread.parent;
            let from = utils::fit_string(e.from.short(), FROM_W);
            let indent = if row.depth == 0 {
                String::new()
            } else {
                format!("{}└ ", "  ".repeat(row.depth - 1))
            };
            let subject = if e.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                e.subject.clone()
            };
            let subject_avail = subject_w.saturating_sub(indent.chars().count());
            let subject_padded = utils::fit_string(&subject, subject_avail);
            let text_style = if row.thread.parent.is_unread() {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let flagged_span = if e.has_mark(Flag::Flagged) {
                Span::styled("★", Style::default().fg(Color::Yellow))
            } else {
                Span::raw(" ")
            };
            let replied_span = if e.has_mark(Flag::Replied) {
                Span::styled("↩", Style::default().fg(Color::Yellow))
            } else {
                Span::raw(" ")
            };
            let passed_span = if e.has_mark(Flag::Passed) {
                Span::styled("→ ", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("  ")
            };
            ListItem::new(Line::from(vec![
                Span::styled(from, text_style),
                Span::raw(" "),
                Span::styled(indent, Style::default().fg(Color::DarkGray)),
                Span::styled(subject_padded, text_style),
                Span::raw(" "),
                flagged_span,
                replied_span,
                passed_span,
                Span::styled(
                    format!("{:<DATE_W$}", humanize_date(e.timestamp)),
                    Style::default().fg(Color::Cyan),
                ),
            ]))
        })
        .collect();

    let widget = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(widget, area, &mut view.state);
}

fn flatten_recursive(threads: &[Rc<EmailThread>], depth: usize, out: &mut Vec<Row>) {
    for thread in threads {
        out.push(Row {
            depth,
            thread: thread.clone(),
        });
        let replies = thread.replies.borrow();
        flatten_recursive(&replies, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::thread::{Email, EmailThread, EmailThreadList};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    fn tlist(threads: Vec<Rc<EmailThread>>) -> EmailThreadList {
        Rc::new(RefCell::new(threads))
    }

    fn make_email(id: &str, from: &str, subject: &str, unread: bool) -> Email {
        let path = if unread {
            PathBuf::from(format!("/mb/new/{}", id))
        } else {
            PathBuf::from(format!("/mb/cur/{}:2,S", id))
        };
        Email::new(id, None, from, subject, Some(1_700_000_000), path)
    }

    fn make_email_with_flags(id: &str, flags: &str) -> Email {
        Email::new(
            id,
            None,
            "sender",
            "Subject",
            Some(1_700_000_000),
            PathBuf::from(format!("/mb/cur/{}:2,{}", id, flags)),
        )
    }

    fn thread(id: &str, from: &str, subject: &str, unread: bool) -> Rc<EmailThread> {
        Rc::new(EmailThread {
            parent: make_email(id, from, subject, unread),
            replies: RefCell::new(Vec::new()),
        })
    }

    fn with_reply(parent: Rc<EmailThread>, child: Rc<EmailThread>) -> Rc<EmailThread> {
        parent.replies.borrow_mut().push(child);
        parent
    }

    fn rendered_lines(view: &mut ThreadsView, w: u16, h: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, ratatui::layout::Rect::new(0, 0, w, h), view);
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

    // ── new ──────────────────────────────────────────────────────────────────

    #[test]
    fn new_empty_has_no_selection() {
        let view = ThreadsView::new(tlist(vec![]));
        assert!(view.selected().is_none());
        assert!(view.rows.is_empty());
    }

    #[test]
    fn new_selects_first_thread() {
        let view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Hello", false),
            thread("b", "Bob", "World", false),
        ]));
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn new_counts_all_rows_including_replies() {
        let parent = with_reply(
            thread("p", "Alice", "Parent", false),
            thread("c", "Bob", "Re: Parent", false),
        );
        let view = ThreadsView::new(tlist(vec![parent]));
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn new_flattens_replies_depth_first() {
        // Tree: A → [B → [C], D]
        let c = thread("c", "", "C", false);
        let b = with_reply(thread("b", "", "B", false), c);
        let d = thread("d", "", "D", false);
        let a = {
            let a = thread("a", "", "A", false);
            a.replies.borrow_mut().push(b);
            a.replies.borrow_mut().push(d);
            a
        };
        let view = ThreadsView::new(tlist(vec![a]));
        // Depth-first: A, B, C, D
        assert_eq!(view.rows.len(), 4);
        let ids: Vec<_> = view
            .rows
            .iter()
            .map(|r| r.thread.parent.message_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
    }

    #[test]
    fn new_assigns_correct_depths() {
        let child = thread("c", "", "Re", false);
        let parent = with_reply(thread("p", "", "Root", false), child);
        let view = ThreadsView::new(tlist(vec![parent]));
        assert_eq!(view.rows[0].depth, 0);
        assert_eq!(view.rows[1].depth, 1);
    }

    // ── invalidate ───────────────────────────────────────────────────────────

    #[test]
    fn invalidate_rebuilds_rows() {
        let list = tlist(vec![thread("a", "", "A", false)]);
        let mut view = ThreadsView::new(list.clone());
        assert_eq!(view.rows.len(), 1);
        list.borrow_mut().push(thread("b", "", "B", false));
        view.invalidate();
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn invalidate_preserves_selection_by_message_id() {
        let list = tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]);
        let mut view = ThreadsView::new(list.clone());
        view.next_email(1); // select "b"
        assert_eq!(view.selected().unwrap().parent.message_id, "b");

        // Refresh with same threads in a different order
        *list.borrow_mut() = vec![
            thread("c", "", "C", false),
            thread("b", "", "B", false),
            thread("a", "", "A", false),
        ];
        view.invalidate();
        assert_eq!(view.selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn invalidate_selects_same_index_when_thread_removed() {
        let list = tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
            thread("d", "", "D", false),
        ]);
        let mut view = ThreadsView::new(list.clone());
        view.next_email(1);
        view.next_email(1); // select index 2 ("c")

        // Remove "c" — the item now at index 2 is "d"
        *list.borrow_mut() = vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("d", "", "D", false),
        ];
        view.invalidate();
        assert_eq!(view.selected().unwrap().parent.message_id, "d");
    }

    #[test]
    fn invalidate_clamps_index_when_list_shrinks() {
        let list = tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]);
        let mut view = ThreadsView::new(list.clone());
        view.next_email(1);
        view.next_email(1); // select index 2 ("c")

        // Shrink to one item — index 2 doesn't exist, clamp to 0
        *list.borrow_mut() = vec![thread("a", "", "A", false)];
        view.invalidate();
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn invalidate_to_empty_clears_selection() {
        let list = tlist(vec![thread("a", "", "A", false)]);
        let mut view = ThreadsView::new(list.clone());
        list.borrow_mut().clear();
        view.invalidate();
        assert!(view.selected().is_none());
    }

    #[test]
    fn invalidate_from_empty_to_nonempty_selects_first() {
        let list = tlist(vec![]);
        let mut view = ThreadsView::new(list.clone());
        list.borrow_mut().push(thread("a", "", "A", false));
        view.invalidate();
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    // ── prev / next ───────────────────────────────────────────────────────────

    #[test]
    fn prev_at_first_stays() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
        ]));
        view.prev_email(1);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn next_moves_to_second() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
        ]));
        view.next_email(1);
        assert_eq!(view.selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn next_at_last_stays() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "", "A", false)]));
        view.next_email(1);
        view.next_email(1);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn prev_after_next_returns_to_first() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
        ]));
        view.next_email(1);
        view.prev_email(1);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn next_on_empty_does_not_panic() {
        let mut view = ThreadsView::new(tlist(vec![]));
        view.next_email(1);
        assert!(view.selected().is_none());
    }

    #[test]
    fn prev_on_empty_does_not_panic() {
        let mut view = ThreadsView::new(tlist(vec![]));
        view.prev_email(1);
        assert!(view.selected().is_none());
    }

    #[test]
    fn next_skips_multiple_steps() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
            thread("d", "", "D", false),
            thread("e", "", "E", false),
        ]));
        view.next_email(3);
        assert_eq!(view.selected().unwrap().parent.message_id, "d");
    }

    #[test]
    fn prev_skips_multiple_steps() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
            thread("d", "", "D", false),
            thread("e", "", "E", false),
        ]));
        view.next_email(4);
        view.prev_email(3);
        assert_eq!(view.selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn next_clamps_at_last() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]));
        view.next_email(100);
        assert_eq!(view.selected().unwrap().parent.message_id, "c");
    }

    #[test]
    fn prev_clamps_at_first() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]));
        view.next_email(2);
        view.prev_email(100);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    // ── first / last email ─────────────────────────────────────────────────

    #[test]
    fn first_email_selects_first() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]));
        view.next_email(1);
        view.next_email(1); // select "c"
        view.first_email();
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn last_email_selects_last() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "", "A", false),
            thread("b", "", "B", false),
            thread("c", "", "C", false),
        ]));
        view.last_email();
        assert_eq!(view.selected().unwrap().parent.message_id, "c");
    }

    #[test]
    fn first_email_on_empty_does_not_panic() {
        let mut view = ThreadsView::new(tlist(vec![]));
        view.first_email();
        assert!(view.selected().is_none());
    }

    #[test]
    fn last_email_on_empty_does_not_panic() {
        let mut view = ThreadsView::new(tlist(vec![]));
        view.last_email();
        assert!(view.selected().is_none());
    }

    // ── show_unread / show_all / toggle_unread ─────────────────────────────

    #[test]
    fn show_unread_filters_read_emails() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Unread", true),
            thread("c", "Carol", "Also read", false),
        ]));
        view.toggle_unread();
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.selected().unwrap().parent.message_id, "b");
    }

    #[test]
    fn show_all_restores_all_emails() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Unread", true),
        ]));
        view.toggle_unread();
        assert_eq!(view.rows.len(), 1);
        view.toggle_unread();
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn toggle_unread_switches_mode() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Unread", true),
        ]));
        view.toggle_unread();
        assert_eq!(view.rows.len(), 1);
        view.toggle_unread();
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn show_unread_preserves_selection_when_selected_is_unread() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Unread", true),
            thread("c", "Carol", "Also unread", true),
        ]));
        view.next_email(1);
        view.next_email(1); // select "c"
        view.toggle_unread();
        assert_eq!(view.selected().unwrap().parent.message_id, "c");
    }

    #[test]
    fn show_unread_clamps_when_selected_is_read() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Also read", false),
            thread("c", "Carol", "Unread", true),
        ]));
        // select "a" (read) — after filtering, "a" disappears
        view.toggle_unread();
        assert_eq!(view.selected().unwrap().parent.message_id, "c");
    }

    #[test]
    fn show_unread_with_no_unread_clears_selection() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Also read", false),
        ]));
        view.toggle_unread();
        assert!(view.selected().is_none());
        assert!(view.rows.is_empty());
    }

    #[test]
    fn invalidate_respects_unread_filter() {
        let list = tlist(vec![
            thread("a", "Alice", "Read", false),
            thread("b", "Bob", "Unread", true),
        ]);
        let mut view = ThreadsView::new(list.clone());
        view.toggle_unread();
        assert_eq!(view.rows.len(), 1);

        list.borrow_mut()
            .push(thread("c", "Carol", "New unread", true));
        view.invalidate();
        assert_eq!(view.rows.len(), 2);
    }

    // ── search ──────────────────────────────────────────────────────────────

    #[test]
    fn set_search_filters_by_subject() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Rust is great", false),
            thread("b", "Bob", "Python news", false),
            thread("c", "Carol", "Rust update", false),
        ]));
        view.set_search(Some("rust"));
        assert_eq!(view.rows.len(), 2);
        let ids: Vec<_> = view
            .rows
            .iter()
            .map(|r| r.thread.parent.message_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "HELLO World", false),
            thread("b", "Bob", "goodbye", false),
        ]));
        view.set_search(Some("hello"));
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn clear_search_restores_all() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Rust", false),
            thread("b", "Bob", "Python", false),
        ]));
        view.set_search(Some("rust"));
        assert_eq!(view.rows.len(), 1);
        view.set_search(None);
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn search_returns_current_query() {
        let mut view = ThreadsView::new(tlist(vec![]));
        assert!(view.search().is_none());
        view.set_search(Some("test"));
        assert_eq!(view.search(), Some("test"));
        view.set_search(None);
        assert!(view.search().is_none());
    }

    #[test]
    fn search_combines_with_unread_filter() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Rust news", true),
            thread("b", "Bob", "Rust update", false),
            thread("c", "Carol", "Python news", true),
        ]));
        view.toggle_unread();
        view.set_search(Some("rust"));
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.selected().unwrap().parent.message_id, "a");
    }

    #[test]
    fn search_no_match_clears_selection() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "Alice", "Hello", false)]));
        view.set_search(Some("zzzzz"));
        assert!(view.rows.is_empty());
        assert!(view.selected().is_none());
    }

    // ── draw ─────────────────────────────────────────────────────────────────

    #[test]
    fn draw_shows_threads_content() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "Alice", "Hello", false)]));
        let content: String = rendered_lines(&mut view, 60, 5).join("\n");
        assert!(content.contains("Alice"), "got:\n{content}");
    }

    #[test]
    fn draw_shows_search_filters_results() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "Hello", false),
            thread("b", "Bob", "World", false),
        ]));
        view.set_search(Some("hello"));
        let content: String = rendered_lines(&mut view, 60, 5).join("\n");
        assert!(content.contains("Alice"), "got:\n{content}");
        assert!(!content.contains("Bob"), "got:\n{content}");
    }

    #[test]
    fn draw_renders_from_and_subject() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "Alice", "Hello world", false)]));
        let content: String = rendered_lines(&mut view, 80, 5).join("\n");
        assert!(content.contains("Alice"), "got:\n{content}");
        assert!(content.contains("Hello world"), "got:\n{content}");
    }

    #[test]
    fn draw_uses_no_subject_placeholder() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "Alice", "", false)]));
        let content: String = rendered_lines(&mut view, 80, 5).join("\n");
        assert!(content.contains("(no subject)"), "got:\n{content}");
    }

    #[test]
    fn draw_renders_multiple_threads() {
        let mut view = ThreadsView::new(tlist(vec![
            thread("a", "Alice", "First", false),
            thread("b", "Bob", "Second", false),
        ]));
        let content: String = rendered_lines(&mut view, 80, 6).join("\n");
        assert!(content.contains("Alice"), "got:\n{content}");
        assert!(content.contains("Bob"), "got:\n{content}");
        assert!(content.contains("First"), "got:\n{content}");
        assert!(content.contains("Second"), "got:\n{content}");
    }

    #[test]
    fn draw_shows_replied_indicator() {
        let email = make_email_with_flags("a", "RS");
        let t = Rc::new(EmailThread {
            parent: email,
            replies: RefCell::new(vec![]),
        });
        let mut view = ThreadsView::new(tlist(vec![t]));
        let content: String = rendered_lines(&mut view, 80, 3).join("\n");
        assert!(
            content.contains('↩'),
            "replied indicator missing:\n{content}"
        );
    }

    #[test]
    fn draw_shows_passed_indicator() {
        let email = make_email_with_flags("a", "PS");
        let t = Rc::new(EmailThread {
            parent: email,
            replies: RefCell::new(vec![]),
        });
        let mut view = ThreadsView::new(tlist(vec![t]));
        let content: String = rendered_lines(&mut view, 80, 3).join("\n");
        assert!(
            content.contains('→'),
            "passed indicator missing:\n{content}"
        );
    }

    #[test]
    fn draw_shows_flagged_indicator() {
        let email = make_email_with_flags("a", "FS");
        let t = Rc::new(EmailThread {
            parent: email,
            replies: RefCell::new(vec![]),
        });
        let mut view = ThreadsView::new(tlist(vec![t]));
        let content: String = rendered_lines(&mut view, 80, 3).join("\n");
        assert!(
            content.contains('★'),
            "flagged indicator missing:\n{content}"
        );
    }

    #[test]
    fn draw_shows_no_indicators_for_plain_email() {
        let mut view = ThreadsView::new(tlist(vec![thread("a", "Alice", "Hello", false)]));
        let content: String = rendered_lines(&mut view, 80, 3).join("\n");
        assert!(
            !content.contains('↩'),
            "unexpected replied indicator:\n{content}"
        );
        assert!(
            !content.contains('→'),
            "unexpected passed indicator:\n{content}"
        );
        assert!(
            !content.contains('★'),
            "unexpected flagged indicator:\n{content}"
        );
    }
}
