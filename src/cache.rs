use anyhow::Result;
use mail_parser::mailbox::maildir::MessageIterator;
use mail_parser::{Message, MessageParser};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

type OwnedMessage = Message<'static>;

#[derive(Debug, PartialEq)]
pub struct EmailThread {
    data: Rc<OwnedMessage>,
    replies: RefCell<Vec<Rc<EmailThread>>>,
}

impl EmailThread {
    pub fn new(m: &Rc<OwnedMessage>) -> Self {
        Self {
            data: m.clone(),
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
        if let Some(content) = MessageParser::default().parse(msg?.contents()) {
            let email: Rc<OwnedMessage> = Rc::new(content.into_owned());
            let thread = Rc::new(EmailThread::new(&email));
            let header = email.in_reply_to();

            if header.is_empty() || header.as_text().is_none() {
                threads.push(thread.clone());
            } else {
                let reply_to_id = header.as_text().unwrap().to_string();

                if let Some(parent) = lookup.get_mut(&reply_to_id) {
                    // if we already encountered the parent, we just add this
                    // email to its children
                    parent.replies.borrow_mut().push(thread.clone());
                } else {
                    if let Some(queue) = searching.get_mut(&reply_to_id) {
                        // someone is already searching for our parent, so we
                        // add this email to the queue
                        queue.push(thread.clone());
                    } else {
                        // this is the first time we are searching for the parent.
                        // create the list of children searching for it
                        searching.insert(reply_to_id.clone(), vec![thread.clone()]);
                    }
                }
            }

            let id = email.message_id().expect("Empty message ID");

            if let Some(children) = searching.get_mut(id) {
                // we found a list of children searching for our email, so we add
                // them to the email's children list
                children
                    .into_iter()
                    .for_each(|c| thread.replies.borrow_mut().push(c.clone()));

                // remove this email from the list
                searching.remove_entry(id);
            }

            // save the email inside our emails hash lookup
            lookup.insert(id.to_string(), thread.clone());
        }
    }

    // leftover emails without parents will be considered parents
    searching
        .into_iter()
        .for_each(|(_, k)| k.into_iter().for_each(|i| threads.push(i)));

    Ok(threads)
}

fn print_threads(counter: usize, thread: &Rc<EmailThread>) {
    println!(
        "{}{}{}",
        " ".repeat(2 * counter),
        if counter > 0 { "` " } else { "" },
        thread.data.subject().unwrap_or_default()
    );

    for t in thread.replies.borrow().iter() {
        print_threads(counter + 1, &t);
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
            print_threads(0 as usize, &t);
        }

        dbg!(threads.len());
    }
}
