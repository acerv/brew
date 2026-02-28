use anyhow::Result;
use mail_parser::MessageParser;
use mail_parser::mailbox::maildir::MessageIterator;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, PartialEq)]
pub struct EmailMeta {
    pub message_id: String,
    pub subject: String,
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

/// Function that generates a list of threads from a Maildir folder.
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
///
pub fn build_threads(dir: &str) -> Result<Vec<Rc<EmailThread>>> {
    let mut searching: HashMap<String, Vec<Rc<EmailThread>>> = HashMap::new();
    let mut lookup: HashMap<String, Rc<EmailThread>> = HashMap::new();
    let mut threads: Vec<Rc<EmailThread>> = Vec::new();

    for msg in MessageIterator::new(PathBuf::from(dir))? {
        let content = msg?;
        // Parse only headers — skip body decoding entirely.
        let parsed = match MessageParser::default().parse_headers(content.contents()) {
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

    Ok(threads)
}

#[cfg(test)]
fn print_threads(prefix: &str, thread: &EmailThread) {
    println!("{}{}", prefix, thread.data.subject);
    let child_prefix = format!("{}  ` ", prefix);
    for child in thread.replies.borrow().iter() {
        print_threads(&child_prefix, child);
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_threads() {
        let threads = build_threads("/home/acer/Mail/LTP/").unwrap();
        for t in threads.iter() {
            print_threads("", t);
        }
        dbg!(threads.len());
    }
}
