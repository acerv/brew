// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::thread::{Email, EmailThread, EmailThreadList};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Sort order for the thread list.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum SortOrder {
    #[default]
    Descending,
    Ascending,
}

impl SortOrder {
    pub fn toggle(self) -> Self {
        match self {
            SortOrder::Descending => SortOrder::Ascending,
            SortOrder::Ascending => SortOrder::Descending,
        }
    }
}

/// Holds the thread tree built from a Maildir folder.
///
/// Only headers are parsed during the scan (O(n) over the mailbox). The
/// file path for each message is stored so that any individual email can be
/// loaded on demand without another directory walk.
///
/// Email files might be read in a random order, so we can't really create a
/// list of threads in `O(n)`, where `n` is the number of emails, unless we
/// reserve a slot for each parent we didn't reach yet.
///
/// For instance, let's suppose that we would like to obtain the following thread:
///
/// ```text
/// A -- B -- C
///  `-- D -- E -- F
///            `-- G
/// ```
///
/// Unfortunately, we might receive `E` before `A` and `D`, so it becomes hard
/// to guess where we can find the parents:
///
/// ```text
/// ? -- B -- C
///  `-- ? -- E -- F
///            `-- G
/// ```
///
/// By using a Hash we can store parents IDs we are searching for and the list
/// of children which are searching for it. We will probably need a new hash
/// also for storing the list of emails we already seen. In this way, every
/// search operation will cost `O(1)`.
///
/// - `Hs` associates parent ID to a list of emails which are searching for it
/// - `Hm` is the lookup hash of already seen messages, associating `Message-ID`
///   to its message
///
/// We can define a parent as an email with empty `reply_to`, hence:
///
/// - `Lp` is the list of parents
///
/// The insertion algorithm will look like the following:
///
/// 1. we read a new email `Mi`
/// 2. we extract `reply_to` and if
///    - it's empty, we add it to `Lp`
///    - it's non empty
///      - we extract `Hm[reply_to]`
///        - if it's empty, we add it to `Hs[reply_to]`
///        - if it's not empty, we add `Mi` to `Hm[reply_to]` children
/// 3. we extract `id` from `Mi` and if `Hs[id]`
///    - is not empty, we set `Hs[id]` as children of `Mi`
///    - is empty, we don't do anything
/// 4. we add `Mi` to `Hm[id]`
///
/// This insertion algorithm has to be repeated for all the emails we read from
/// the Maildir folder. By using `Rc` reference counter, we ensure that messages
/// are stored only once and the memory consumption will be proportional to the
/// amount of emails which have been loaded.
#[derive(Default)]
pub struct Maildir {
    dir: String,
    searching: HashMap<String, Vec<Rc<EmailThread>>>,
    lookup: HashMap<String, Rc<EmailThread>>,
    threads: EmailThreadList,
    sort_order: SortOrder,
}

impl Maildir {
    pub fn path(&self) -> &str {
        &self.dir
    }

    /// Scan `dir` and build the email thread tree.
    pub fn new(dir: &str) -> Result<Self> {
        let mut maildir = Maildir {
            dir: dir.to_string(),
            ..Maildir::default()
        };

        for path in Maildir::iter_maildir(Path::new(dir)) {
            maildir.insert(&path);
        }
        maildir.invalidate();

        Ok(maildir)
    }

    /// Return the shared thread list handle.
    ///
    /// Cloning the returned `ThreadList` is cheap (just bumps the `Rc` ref-count)
    /// and gives the caller shared ownership of the same underlying data — any
    /// subsequent `insert`/`remove`/`invalidate` call will be visible through all
    /// clones without a rebuild.
    pub fn threads(&self) -> EmailThreadList {
        self.threads.clone()
    }

    /// Parse headers from a single new Maildir file and insert it into the
    /// thread tree. A no-op if the file cannot be read or has no `Message-ID`.
    ///
    /// This is the incremental counterpart to `build`: instead of re-scanning
    /// the whole directory, only the one new file is processed.
    ///
    /// Remember to run `invalidate()` once the insert operations are completed.
    pub fn insert(&mut self, path: &Path) {
        let email = match Email::from_file(&path.to_path_buf()) {
            Ok(e) => e,
            Err(_) => return,
        };

        // Skip if we already know this Message-ID (duplicate file).
        if self.lookup.contains_key(&email.message_id) {
            return;
        }

        let reply_to = &email.reply_to.clone();
        let thread = Rc::new(EmailThread::new(email));

        // Step 2: link to parent or queue for later.
        if let Some(reply_to_id) = &reply_to {
            let reply_to_id = reply_to_id.to_string();
            if let Some(parent) = self.lookup.get(&reply_to_id) {
                // Parent already seen — attach directly.
                parent.replies.borrow_mut().push(thread.clone());
            } else {
                // Parent not yet seen — queue under its ID.
                self.searching
                    .entry(reply_to_id)
                    .or_default()
                    .push(thread.clone());
            }
        } else {
            // No In-Reply-To — this is a root thread.
            self.threads.borrow_mut().push(thread.clone());
        }

        // Step 3: attach any children that were queued waiting for us.
        if let Some(children) = self.searching.remove(&thread.parent.message_id) {
            thread.replies.borrow_mut().extend(children);
        }

        // Step 4: register this message.
        self.lookup
            .insert(thread.parent.message_id.clone(), thread.clone());
    }

    /// Validate `content` as a well-formed RFC 2822 email and write it into
    /// `dir/new/`. Returns an error if the content cannot be parsed or is
    /// missing required headers.
    pub fn write_email(&self, content: &str) -> Result<()> {
        let msg = mail_parser::MessageParser::default()
            .parse(content.as_bytes())
            .ok_or_else(|| anyhow::anyhow!("invalid email format"))?;
        if msg.from().is_none() {
            return Err(anyhow::anyhow!("missing From header"));
        }
        let new_dir = Path::new(&self.dir).join("new");
        std::fs::create_dir_all(&new_dir)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let pid = std::process::id();
        let filename = format!("{timestamp}.{pid}.localhost");
        std::fs::write(new_dir.join(filename), content)?;
        Ok(())
    }

    /// Look up a thread by its root message-id. Returns `None` if not found.
    pub fn find_by_id(&self, message_id: &str) -> Option<&Rc<EmailThread>> {
        self.lookup.get(message_id)
    }

    /// Total number of emails tracked in this mailbox.
    pub fn email_count(&self) -> usize {
        self.lookup.len()
    }

    /// Count the number of unread emails across all threads in this mailbox.
    pub fn unread_count(&self) -> usize {
        self.lookup
            .values()
            .filter(|t| t.parent.is_unread())
            .count()
    }

    /// Remove the thread node whose on-disk path matches `path` from the tree,
    /// along with all its children.  The corresponding entries are also removed
    /// from the internal `lookup` table so they can be re-inserted later.
    /// A no-op if no node matches.
    ///
    /// Remember to run `invalidate()` once the remove operations are completed.
    pub fn remove(&mut self, path: &Path) {
        let mut removed_ids = Vec::new();
        remove_thread_by_path(&mut self.threads.borrow_mut(), path, &mut removed_ids);
        for id in removed_ids {
            self.lookup.remove(&id);
        }
    }

    /// Remove the thread whose root email has the given `message_id` from both
    /// the in-memory tree and disk.  A no-op if no matching thread is found.
    ///
    /// Remember to run `invalidate()` once the remove operations are completed.
    pub fn remove_by_id(&mut self, message_id: &str) {
        let path = match self.lookup.get(message_id) {
            Some(t) => t.parent.path().clone(),
            None => return,
        };
        let _ = std::fs::remove_file(&path);
        self.remove(&path);
    }

    /// Synchronise the in-memory state against the current contents of `dir`.
    ///
    /// - Files that are on disk but not in `lookup` are inserted.
    /// - Files that are in `lookup` but no longer on disk are removed.
    ///
    /// A single `invalidate()` is called at the end to promote orphans and
    /// re-sort the full tree.  This method is idempotent: calling it when
    /// nothing has changed on disk is a no-op (aside from the sort pass).
    pub fn sync(&mut self) {
        let disk: HashSet<PathBuf> = Maildir::iter_maildir(Path::new(&self.dir)).collect();

        // First pass: remove emails that disappeared from disk.  Removing a
        // parent also evicts its descendants from `lookup`, even though their
        // files may still exist — so we recompute `known` afterwards.
        let known: HashSet<PathBuf> = self
            .lookup
            .values()
            .map(|t| t.parent.path().clone())
            .collect();
        for gone in known.difference(&disk) {
            self.remove(gone);
        }

        // Second pass: insert emails not currently tracked.  Uses the updated
        // `lookup` so that children evicted by a parent removal are picked up.
        let known_after: HashSet<PathBuf> = self
            .lookup
            .values()
            .map(|t| t.parent.path().clone())
            .collect();
        for arrived in disk.difference(&known_after) {
            self.insert(arrived);
        }

        self.invalidate();
    }

    /// Set the sort order and re-sort immediately.
    pub fn set_sort_order(&mut self, order: SortOrder) {
        self.sort_order = order;
        self.invalidate();
    }

    /// Return the current sort order.
    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    /// Invalidate the current tables and re-sort all the threads.
    pub fn invalidate(&mut self) {
        let mut threads = self.threads.borrow_mut();

        // Orphaned emails (parent never found) become root threads.
        for orphans in self.searching.values() {
            threads.extend(orphans.clone());
        }
        self.searching.clear();

        sort_threads(&mut threads, self.sort_order);
    }

    /// Iterate over all emails inside maildir. This is used instead of the
    /// `MessageIterator`, so we have control over the return data that is
    /// used in order to create `Email` via `from_file()` method.
    fn iter_maildir(dir: &Path) -> impl Iterator<Item = PathBuf> {
        ["new", "cur"].iter().flat_map(move |sub| {
            std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
        })
    }
}

////////////////////////////////////////////////////////////////////////////////

/// Recursively remove the first node whose path equals `path`, collecting the
/// message-ids of the removed node and all its descendants into `removed`.
fn remove_thread_by_path(
    threads: &mut Vec<Rc<EmailThread>>,
    path: &Path,
    removed: &mut Vec<String>,
) {
    if let Some(pos) = threads.iter().position(|t| *t.parent.path() == *path) {
        let node = threads.remove(pos);
        collect_ids(&node, removed);
        return;
    }
    for t in threads.iter() {
        remove_thread_by_path(&mut t.replies.borrow_mut(), path, removed);
    }
}

/// Collect the message-id of `thread` and all its descendants.
fn collect_ids(thread: &Rc<EmailThread>, out: &mut Vec<String>) {
    out.push(thread.parent.message_id.clone());
    for reply in thread.replies.borrow().iter() {
        collect_ids(reply, out);
    }
}

/// Return the most recent timestamp across `thread` and all its replies.
/// Returns `None` if no email in the tree has a timestamp.
fn thread_max_timestamp(thread: &Rc<EmailThread>) -> Option<i64> {
    let mut max = thread.parent.timestamp;
    for reply in thread.replies.borrow().iter() {
        let t = thread_max_timestamp(reply);
        max = match (max, t) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }
    max
}

/// Sort `threads` by the most recent timestamp in each thread tree, then
/// recurse into replies. Emails with no timestamp sort after all dated ones.
fn sort_threads(threads: &mut [Rc<EmailThread>], order: SortOrder) {
    threads.sort_unstable_by(|a, b| {
        let ta = thread_max_timestamp(a);
        let tb = thread_max_timestamp(b);
        match order {
            SortOrder::Descending => tb.cmp(&ta),
            SortOrder::Ascending => ta.cmp(&tb),
        }
    });
    for thread in threads.iter() {
        sort_threads(&mut thread.replies.borrow_mut(), order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    /// Create a fresh Maildir skeleton in a temp directory.
    fn make_maildir() -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("brew-maildir-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(dir.join("new")).unwrap();
        std::fs::create_dir_all(dir.join("cur")).unwrap();
        dir
    }

    /// Build minimal RFC 2822 email content.
    fn email_content(message_id: &str, in_reply_to: Option<&str>, date: &str) -> String {
        let mut s = format!(
            "Message-ID: <{}>\r\nFrom: Test <test@example.com>\r\nSubject: Test\r\nDate: {}\r\n",
            message_id, date
        );
        if let Some(reply) = in_reply_to {
            s.push_str(&format!("In-Reply-To: <{}>\r\n", reply));
        }
        s.push_str("\r\nBody\r\n");
        s
    }

    fn write_msg(dir: &Path, subdir: &str, name: &str, content: &str) -> PathBuf {
        let path = dir.join(subdir).join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    // ── Maildir::default ───────────────────────────────────────────────────────

    #[test]
    fn empty_maildir_has_no_threads() {
        let maildir = Maildir::default();
        assert!(maildir.threads().borrow().is_empty());
    }

    // ── Maildir::build ─────────────────────────────────────────────────────────

    #[test]
    fn build_empty_maildir_has_no_threads() {
        let dir = make_maildir();
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert!(maildir.threads().borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_loads_email_from_new() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "id1@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_loads_email_from_cur() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "cur",
            "msg1:2,S",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "id1@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_loads_from_both_new_and_cur() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "cur",
            "msg2:2,S",
            &email_content("id2@test", None, "Tue, 02 Jan 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::insert ────────────────────────────────────────────────────────

    #[test]
    fn insert_single_becomes_root_thread() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&path);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "id1@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn insert_duplicate_message_id_is_skipped() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("dup@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&path);
        maildir.insert(&path);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn insert_reply_parent_first_links_as_child() {
        let dir = make_maildir();
        let parent = write_msg(
            &dir,
            "new",
            "parent",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let child = write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("parent@test"),
                "Tue, 02 Jan 2024 00:00:00 +0000",
            ),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&parent);
        maildir.insert(&child);
        maildir.invalidate();
        assert_eq!(
            maildir.threads().borrow().len(),
            1,
            "child must be attached to parent, not a separate root"
        );
        let threads = maildir.threads();
        let threads = threads.borrow();
        let replies = threads[0].replies.borrow();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].parent.message_id, "child@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn insert_reply_child_first_links_after_parent_inserted() {
        let dir = make_maildir();
        let child = write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("parent@test"),
                "Tue, 02 Jan 2024 00:00:00 +0000",
            ),
        );
        let parent = write_msg(
            &dir,
            "new",
            "parent",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&child);
        maildir.insert(&parent);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        let threads = maildir.threads();
        let threads = threads.borrow();
        let replies = threads[0].replies.borrow();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].parent.message_id, "child@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::remove ────────────────────────────────────────────────────────

    #[test]
    fn remove_root_thread() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&path);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        maildir.remove(&path);
        assert!(maildir.threads().borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_reply_thread() {
        let dir = make_maildir();
        let parent = write_msg(
            &dir,
            "new",
            "parent",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let child = write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("parent@test"),
                "Tue, 02 Jan 2024 00:00:00 +0000",
            ),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&parent);
        maildir.insert(&child);
        maildir.invalidate();
        maildir.remove(&child);
        assert_eq!(maildir.threads().borrow().len(), 1);
        let threads = maildir.threads();
        let threads = threads.borrow();
        let replies = threads[0].replies.borrow();
        assert!(replies.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_nonexistent_path_is_noop() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&path);
        maildir.invalidate();
        maildir.remove(Path::new("/nonexistent/path/that/does/not/exist"));
        assert_eq!(maildir.threads().borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::invalidate ────────────────────────────────────────────────────

    #[test]
    fn invalidate_promotes_orphan_to_root() {
        let dir = make_maildir();
        let child = write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("ghost@test"),
                "Mon, 01 Jan 2024 00:00:00 +0000",
            ),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&child);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "child@test"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalidate_sorts_latest_first() {
        let dir = make_maildir();
        let older = write_msg(
            &dir,
            "new",
            "older",
            &email_content("older@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let newer = write_msg(
            &dir,
            "new",
            "newer",
            &email_content("newer@test", None, "Wed, 01 Jan 2025 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&older);
        maildir.insert(&newer);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 2);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "newer@test"
        );
        assert_eq!(
            maildir.threads().borrow()[1].parent.message_id,
            "older@test"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::remove (lookup cleanup) ──────────────────────────────────────

    #[test]
    fn remove_cleans_lookup_allowing_reinsert() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("reinsert@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&path);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);

        // Remove and verify gone
        maildir.remove(&path);
        assert!(maildir.threads().borrow().is_empty());

        // Re-insert the same message-id — must not be skipped as duplicate
        maildir.insert(&path);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_cleans_lookup_for_descendants() {
        let dir = make_maildir();
        let parent = write_msg(
            &dir,
            "new",
            "parent",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let child = write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("parent@test"),
                "Tue, 02 Jan 2024 00:00:00 +0000",
            ),
        );
        let mut maildir = Maildir::default();
        maildir.insert(&parent);
        maildir.insert(&child);
        maildir.invalidate();

        // Remove parent — child disappears from the tree too
        maildir.remove(&parent);
        assert!(maildir.threads().borrow().is_empty());

        // Both parent and child must be cleaned from lookup
        maildir.insert(&parent);
        maildir.insert(&child);
        maildir.invalidate();
        assert_eq!(maildir.threads().borrow().len(), 1);
        let replies = maildir.threads().borrow()[0].replies.borrow().len();
        assert_eq!(replies, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::sync ─────────────────────────────────────────────────────────

    #[test]
    fn sync_inserts_new_file() {
        let dir = make_maildir();
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert!(maildir.threads().borrow().is_empty());

        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        maildir.sync();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "id1@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_removes_deleted_file() {
        let dir = make_maildir();
        let path = write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);

        std::fs::remove_file(&path).unwrap();
        maildir.sync();
        assert!(maildir.threads().borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_no_change_is_noop() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("id1@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);

        maildir.sync();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "id1@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_handles_insert_and_remove_in_same_pass() {
        let dir = make_maildir();
        let path_a = write_msg(
            &dir,
            "new",
            "msg_a",
            &email_content("a@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);

        // Delete A, add B
        std::fs::remove_file(&path_a).unwrap();
        write_msg(
            &dir,
            "new",
            "msg_b",
            &email_content("b@test", None, "Tue, 02 Jan 2024 00:00:00 +0000"),
        );
        maildir.sync();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].parent.message_id, "b@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_preserves_thread_structure_after_remove_and_reinsert() {
        let dir = make_maildir();
        let parent = write_msg(
            &dir,
            "new",
            "parent",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "new",
            "child",
            &email_content(
                "child@test",
                Some("parent@test"),
                "Tue, 02 Jan 2024 00:00:00 +0000",
            ),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(maildir.threads().borrow()[0].replies.borrow().len(), 1);

        // Remove parent from disk
        std::fs::remove_file(&parent).unwrap();
        maildir.sync();

        // Child is now a root (orphaned)
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "child@test"
        );

        // Re-add parent — it can't retroactively re-attach the already-promoted child
        write_msg(
            &dir,
            "new",
            "parent2",
            &email_content("parent@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        maildir.sync();
        assert_eq!(maildir.threads().borrow().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::write_email ───────────────────────────────────────────────

    #[test]
    fn write_email_creates_file_in_new() {
        let dir = make_maildir();
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let content = email_content("draft@test", None, "Mon, 01 Jan 2024 00:00:00 +0000");
        maildir.write_email(&content).unwrap();
        let new_dir = dir.join("new");
        let files: Vec<_> = std::fs::read_dir(&new_dir).unwrap().collect();
        assert_eq!(files.len(), 1, "expected one file in new/");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_email_is_visible_after_sync() {
        let dir = make_maildir();
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let content = email_content("draft@test", None, "Mon, 01 Jan 2024 00:00:00 +0000");
        maildir.write_email(&content).unwrap();
        maildir.sync();
        assert_eq!(maildir.threads().borrow().len(), 1);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "draft@test"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_email_rejects_missing_from_header() {
        let dir = make_maildir();
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let content = "Message-ID: <id@test>\r\nSubject: No From\r\n\r\nBody\r\n";
        assert!(maildir.write_email(content).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_email_rejects_unparseable_content() {
        let dir = make_maildir();
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert!(maildir.write_email("").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Maildir::find_by_id ────────────────────────────────────────────────────

    #[test]
    fn find_by_id_returns_thread_for_known_id() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("find@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let result = maildir.find_by_id("find@test");
        assert!(result.is_some());
        assert_eq!(result.unwrap().parent.message_id, "find@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_by_id_returns_none_for_unknown_id() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "msg1",
            &email_content("exists@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert!(maildir.find_by_id("ghost@test").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_by_id_returns_none_on_empty_maildir() {
        let dir = make_maildir();
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert!(maildir.find_by_id("any@test").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── SortOrder ─────────────────────────────────────────────────────────────

    #[test]
    fn sort_order_default_is_descending() {
        assert_eq!(SortOrder::default(), SortOrder::Descending);
    }

    #[test]
    fn sort_order_toggle_descending_gives_ascending() {
        assert_eq!(SortOrder::Descending.toggle(), SortOrder::Ascending);
    }

    #[test]
    fn sort_order_toggle_ascending_gives_descending() {
        assert_eq!(SortOrder::Ascending.toggle(), SortOrder::Descending);
    }

    // ── sort_threads / thread_max_timestamp ───────────────────────────────────

    #[test]
    fn descending_sort_puts_newest_thread_first() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "older",
            &email_content("older@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "new",
            "newer",
            &email_content("newer@test", None, "Wed, 01 Jan 2025 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let threads = maildir.threads();
        let threads = threads.borrow();
        assert_eq!(threads[0].parent.message_id, "newer@test");
        assert_eq!(threads[1].parent.message_id, "older@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ascending_sort_puts_oldest_thread_first() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "older",
            &email_content("older@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "new",
            "newer",
            &email_content("newer@test", None, "Wed, 01 Jan 2025 00:00:00 +0000"),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        maildir.set_sort_order(SortOrder::Ascending);
        let threads = maildir.threads();
        let threads = threads.borrow();
        assert_eq!(threads[0].parent.message_id, "older@test");
        assert_eq!(threads[1].parent.message_id, "newer@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sort_uses_most_recent_reply_not_root_date() {
        let dir = make_maildir();
        // Root A is older, but has a very recent reply — should sort first in descending
        write_msg(
            &dir,
            "new",
            "root_a",
            &email_content("root_a@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "new",
            "reply_a",
            &email_content(
                "reply_a@test",
                Some("root_a@test"),
                "Wed, 01 Jan 2025 00:00:00 +0000",
            ),
        );
        // Root B is newer than root_a but has no replies
        write_msg(
            &dir,
            "new",
            "root_b",
            &email_content("root_b@test", None, "Tue, 01 Jun 2024 00:00:00 +0000"),
        );
        let maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        let threads = maildir.threads();
        let threads = threads.borrow();
        // Thread A's max timestamp (reply: 2025) > Thread B's max timestamp (root: Jun 2024)
        assert_eq!(
            threads[0].parent.message_id, "root_a@test",
            "thread with newest reply must come first"
        );
        assert_eq!(threads[1].parent.message_id, "root_b@test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_sort_order_re_sorts_existing_threads() {
        let dir = make_maildir();
        write_msg(
            &dir,
            "new",
            "older",
            &email_content("older@test", None, "Mon, 01 Jan 2024 00:00:00 +0000"),
        );
        write_msg(
            &dir,
            "new",
            "newer",
            &email_content("newer@test", None, "Wed, 01 Jan 2025 00:00:00 +0000"),
        );
        let mut maildir = Maildir::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "newer@test"
        );
        maildir.set_sort_order(SortOrder::Ascending);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "older@test"
        );
        maildir.set_sort_order(SortOrder::Descending);
        assert_eq!(
            maildir.threads().borrow()[0].parent.message_id,
            "newer@test"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
