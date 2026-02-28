use anyhow::{Context, Result};
use mail_parser::mailbox::maildir::MessageIterator;
use mail_parser::{Message, MessageParser};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

/// Only the header fields required for threading and display, plus the path to
/// the on-disk file so that the full message can be loaded on demand in O(1).
#[derive(Debug, PartialEq)]
pub struct EmailMeta {
    pub message_id: String,
    pub subject: String,
    /// Unix timestamp (seconds since epoch) from the `Date:` header.
    /// `None` when the header is absent or unparseable.
    pub timestamp: Option<i64>,
    /// Absolute path to the Maildir file that contains this message.
    /// Passed to `MailCache::load_mail` to retrieve the full email in O(1).
    pub path: PathBuf,
}

#[derive(Debug, PartialEq)]
pub struct EmailThread {
    pub data: EmailMeta,
    pub replies: RefCell<Vec<Rc<EmailThread>>>,
}

impl EmailThread {
    fn new(meta: EmailMeta) -> Self {
        Self {
            data: meta,
            replies: RefCell::new(Vec::new()),
        }
    }
}

/// Sort `threads` descending by timestamp (latest first), then recurse into
/// replies. Emails with no timestamp sort after all dated ones.
fn sort_threads(threads: &mut [Rc<EmailThread>]) {
    threads.sort_unstable_by(|a, b| {
        // None (no date) sorts last.
        b.data.timestamp.cmp(&a.data.timestamp)
    });
    for thread in threads.iter() {
        sort_threads(&mut thread.replies.borrow_mut());
    }
}

/// Holds the thread tree built from a Maildir folder.
///
/// Use `MailCache::build` to construct it, then `load_mail` to fetch the full
/// parsed content of any individual message in O(1).
pub struct MailCache {
    pub threads: Vec<Rc<EmailThread>>,
}

impl MailCache {
    /// Scan `dir` and build the email thread tree.
    ///
    /// Only headers are parsed during the scan (O(n) over the mailbox). The
    /// file path for each message is stored inside [`EmailMeta`] so that any
    /// individual email can be loaded on demand without another directory walk.
    ///
    /// Email files might be read in a random order, so we can't really create a list
    /// of threads in `O(n)`, where `n` is the number of emails, unless we reserve
    /// a slot for each parent we didn't reach yet.
    ///
    /// For instance, let's suppose that we would like to obtain the following thread:
    ///
    /// ```text
    /// A -- B -- C
    ///  `-- D -- E -- F
    ///            `-- G
    /// ```
    ///
    /// Unfortunately, we might receive `E` before `A` and `D`, so it becomes hard to
    /// guess where we can find the parents:
    ///
    /// ```text
    /// ? -- B -- C
    ///  `-- ? -- E -- F
    ///            `-- G
    /// ```
    ///
    /// By using a Hash we can store parents IDs we are searching for and the list of
    /// children which are searching for it. We will probably need a new hash also for
    /// storing the list of emails we already seen. In this way, every search operation
    /// will cost `O(1)`.
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
    /// the Maildir folder.
    ///
    /// By using `Rc` reference counter, we ensure that messages are stored only
    /// once and the memory consumption will be proportional to the amount of emails
    /// which have been loaded.
    pub fn build(dir: &str) -> Result<Self> {
        let mut searching: HashMap<String, Vec<Rc<EmailThread>>> = HashMap::new();
        let mut lookup: HashMap<String, Rc<EmailThread>> = HashMap::new();
        let mut threads: Vec<Rc<EmailThread>> = Vec::new();
        let parser = MessageParser::default();

        for msg in MessageIterator::new(PathBuf::from(dir))? {
            let content = msg?;
            // Parse only headers — skip body decoding entirely.
            let parsed = match parser.parse_headers(content.contents()) {
                Some(p) => p,
                None => continue,
            };

            // Skip emails without a Message-ID; they can't be threaded.
            let id = match parsed.message_id() {
                Some(id) => id.to_string(),
                None => continue,
            };

            let meta = EmailMeta {
                message_id: id.clone(),
                subject: parsed.subject().unwrap_or_default().to_string(),
                timestamp: parsed.date().map(|d| d.to_timestamp()),
                path: content.path().to_path_buf(),
            };

            let thread = Rc::new(EmailThread::new(meta));

            // Step 2: link to parent or queue for later.
            if let Some(reply_to_id) = parsed.in_reply_to().as_text() {
                let reply_to_id = reply_to_id.to_string();
                if let Some(parent) = lookup.get(&reply_to_id) {
                    // Parent already seen — attach directly.
                    parent.replies.borrow_mut().push(thread.clone());
                } else {
                    // Parent not yet seen — queue under its ID.
                    searching
                        .entry(reply_to_id)
                        .or_default()
                        .push(thread.clone());
                }
            } else {
                // No In-Reply-To — this is a root thread.
                threads.push(thread.clone());
            }

            // Step 3: attach any children that were queued waiting for us.
            if let Some(children) = searching.remove(&id) {
                thread.replies.borrow_mut().extend(children);
            }

            // Step 4: register this message.
            lookup.insert(id, thread);
        }

        // Orphaned emails (parent never found) become root threads.
        for (_, orphans) in searching {
            threads.extend(orphans);
        }

        // Sort the full tree — root threads and all reply lists — latest first.
        sort_threads(&mut threads);

        Ok(Self { threads })
    }

    /// Load and fully parse the email described by `meta`.
    ///
    /// The file path is stored inside [`EmailMeta`], so this is an O(1)
    /// operation — no directory scan, no HashMap lookup. It reads the single
    /// file from disk and returns the fully parsed [`Message`].
    pub fn load_mail(meta: &EmailMeta) -> Result<Message<'static>> {
        let bytes = fs::read(&meta.path)
            .with_context(|| format!("failed to read mail file: {}", meta.path.display()))?;

        MessageParser::default()
            .parse(&bytes)
            .map(|m| m.into_owned())
            .with_context(|| format!("failed to parse mail file: {}", meta.path.display()))
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_threads() {
        let cache = MailCache::build("/home/acer/Mail/LTP/").unwrap();
        dbg!(cache.threads.len());
    }

    #[test]
    fn test_load_mail() {
        let cache = MailCache::build("/home/acer/Mail/LTP/").unwrap();
        if let Some(thread) = cache.threads.first() {
            let msg = MailCache::load_mail(&thread.data).unwrap();
            assert_eq!(msg.message_id(), Some(thread.data.message_id.as_str()));
        }
    }
}
