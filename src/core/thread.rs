// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::address::Address;

/// A Maildir flag that can be set, queried, or cleared on an email file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// Default state — absence of the `S` flag (unread).
    Unseen,
    /// `S` — Seen (read).
    Seen,
    /// `R` — Answered (replied).
    Replied,
    /// `P` — Passed (forwarded).
    Passed,
    /// `F` — Flagged (starred).
    Flagged,
}

impl Flag {
    /// The Maildir character for this flag, or `None` for `Unseen`
    /// (which is represented by the absence of `S`).
    fn char(self) -> Option<char> {
        match self {
            Flag::Unseen => None,
            Flag::Seen => Some('S'),
            Flag::Replied => Some('R'),
            Flag::Passed => Some('P'),
            Flag::Flagged => Some('F'),
        }
    }
}
use anyhow::{Context, Result};
use mail_parser::{Message, MessageParser};
use std::cell::{Ref, RefCell};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

/// Email metadata that contains only the essential email information.
/// This structure is used to parse emails from a MailDir folder and to reduce
/// the overload caused by parsing multiple fields, such as body.
#[derive(Debug, Clone, PartialEq)]
pub struct Email {
    /// Unique message identifier.
    pub message_id: String,
    /// Indentifier for the reply-to message.
    pub reply_to: Option<String>,
    /// Sender address parsed from the `From:` header.
    pub from: Address,
    /// Subject of the email.
    pub subject: String,
    /// Unix timestamp (seconds since epoch) from the `Date:` header.
    /// `None` when the header is absent or unparseable.
    pub timestamp: Option<i64>,
    /// Absolute path to the Maildir file that contains this message.
    /// Wrapped in `RefCell` so that `mark_as_read` / `mark_as_unread` can
    /// update the path through a shared `&self` reference (e.g. via `Rc`).
    path: RefCell<PathBuf>,
}

impl Email {
    /// Initialize the structure from an email file.
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let bytes = fs::read(path)?;
        let parser = MessageParser::default();
        let parsed = match parser.parse_headers(&bytes) {
            Some(p) => p,
            None => return Err(anyhow::anyhow!("Can't parse headers")),
        };
        let id = match parsed.message_id() {
            Some(id) => id.to_string(),
            None => return Err(anyhow::anyhow!("No Message-ID")),
        };
        let timestamp = parsed.date().map(|d| d.to_timestamp());
        let from = parsed
            .from()
            .and_then(|a| a.iter().next())
            .map(|a| {
                Address::new(
                    a.name().unwrap_or_default(),
                    a.address().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let reply_to = parsed.in_reply_to().as_text().map(str::to_string);
        let subject = parsed.subject().unwrap_or_default().to_string();

        Ok(Self {
            message_id: id.clone(),
            reply_to,
            from,
            subject,
            timestamp,
            path: RefCell::new(path.to_path_buf()),
        })
    }

    /// Returns the on-disk path of this email file.
    pub fn path(&self) -> Ref<'_, PathBuf> {
        self.path.borrow()
    }

    /// Returns `true` when this email carries `flag`.
    ///
    /// `Flag::Unseen` returns `true` when the email is unread — either because
    /// it sits in `new/` or because the `S` flag is absent in `cur/`.
    /// All other flags check the `:2,flags` section of the filename directly.
    pub fn has_mark(&self, flag: Flag) -> bool {
        if flag == Flag::Unseen {
            // new/ is always unread; cur/ is unread when S is absent.
            if self.path().components().any(|c| c.as_os_str() == "new") {
                return true;
            }
            return !self.has_mark(Flag::Seen);
        }

        let path = self.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(pos) = name.rfind(':')
            && let Some(flags) = name[pos + 1..].strip_prefix("2,")
            && let Some(ch) = flag.char()
        {
            return flags.contains(ch);
        }
        false
    }

    /// Set `flag` on this email, renaming the file on disk.
    ///
    /// `Flag::Unseen` clears the `S` flag (marks the email as unread).
    /// Errors are silently ignored — a failed rename just means the status
    /// won't be synced back to the server, which is not fatal.
    pub fn mark(&self, flag: Flag) {
        if flag == Flag::Unseen {
            self.clear_mark(Flag::Seen);
            return;
        }
        let Some(ch) = flag.char() else { return };

        let path = self.path().clone();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => return,
        };
        let dir = match path.parent() {
            Some(d) => d,
            None => return,
        };

        let new_name = if path.components().any(|c| c.as_os_str() == "new") {
            format!("{}:2,{}", name, ch)
        } else if let Some(colon) = name.rfind(':') {
            let base = &name[..colon];
            let info = &name[colon + 1..];
            if let Some(flags) = info.strip_prefix("2,") {
                if flags.contains(ch) {
                    return; // already set
                }
                let mut chars: Vec<char> = flags.chars().collect();
                chars.push(ch);
                chars.sort_unstable();
                format!("{}:2,{}", base, chars.into_iter().collect::<String>())
            } else {
                format!("{}:2,{}", name, ch)
            }
        } else {
            format!("{}:2,{}", name, ch)
        };

        if new_name == name {
            return;
        }

        let cur_dir = if path.components().any(|c| c.as_os_str() == "new") {
            dir.parent().unwrap_or(dir).join("cur")
        } else {
            dir.to_path_buf()
        };

        let dest = cur_dir.join(&new_name);
        if std::fs::rename(&path, &dest).is_ok() {
            *self.path.borrow_mut() = dest;
        }
    }

    /// Clear `flag` from this email, renaming the file on disk.
    ///
    /// `Flag::Unseen` sets the `S` flag (marks the email as read).
    /// Errors are silently ignored.
    pub fn clear_mark(&self, flag: Flag) {
        if flag == Flag::Unseen {
            self.mark(Flag::Seen);
            return;
        }
        let Some(ch) = flag.char() else { return };

        let path = self.path().clone();

        // Files in new/ have no flags to clear.
        if path.components().any(|c| c.as_os_str() == "new") {
            return;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => return,
        };
        let dir = match path.parent() {
            Some(d) => d,
            None => return,
        };

        let new_name = if let Some(colon) = name.rfind(':') {
            let base = &name[..colon];
            let info = &name[colon + 1..];
            if let Some(flags) = info.strip_prefix("2,") {
                if !flags.contains(ch) {
                    return; // already absent
                }
                let new_flags: String = flags.chars().filter(|&c| c != ch).collect();
                format!("{}:2,{}", base, new_flags)
            } else {
                return;
            }
        } else {
            return;
        };

        if new_name == name {
            return;
        }

        let dest = dir.join(&new_name);
        if std::fs::rename(&path, &dest).is_ok() {
            *self.path.borrow_mut() = dest;
        }
    }

    /// Convenience: returns `true` when email should be considered unread.
    pub fn is_unread(&self) -> bool {
        self.has_mark(Flag::Unseen)
    }

    /// Convert the current `Email` into `Message<'static>`.
    pub fn to_message(&self) -> Result<Message<'static>> {
        let path = self.path();
        let bytes = fs::read(&*path)
            .with_context(|| format!("failed to read mail file: {}", path.display()))?;

        MessageParser::default()
            .parse(&bytes)
            .map(|m| m.into_owned())
            .with_context(|| format!("failed to parse mail file: {}", path.display()))
    }

    /// Construct an `Email` directly. Only available in test builds.
    #[cfg(test)]
    pub fn new(
        message_id: &str,
        reply_to: Option<String>,
        from: &str,
        subject: &str,
        timestamp: Option<i64>,
        path: PathBuf,
    ) -> Self {
        Self {
            message_id: message_id.to_string(),
            reply_to,
            from: Address::new(from, ""),
            subject: subject.to_string(),
            timestamp,
            path: RefCell::new(path),
        }
    }
}

/// Shared, mutable list of root email threads.
///
/// Both `Maildir` and `ThreadsView` hold a clone of the same `Rc`, so any
/// structural change made through `Maildir` (insert/remove/invalidate) is
/// immediately visible to every holder without a rebuild.
pub type EmailThreadList = Rc<RefCell<Vec<Rc<EmailThread>>>>;

/// A node in the email thread tree.
/// Contains email metadata and a list of replies.
#[derive(Debug, PartialEq)]
pub struct EmailThread {
    pub parent: Email,
    pub replies: RefCell<Vec<Rc<EmailThread>>>,
}

impl EmailThread {
    pub fn new(meta: Email) -> Self {
        Self {
            parent: meta,
            replies: RefCell::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("brew-thread-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(dir.join("new")).unwrap();
        std::fs::create_dir_all(dir.join("cur")).unwrap();
        dir
    }

    fn write_file(path: &PathBuf, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    fn minimal_email(
        message_id: &str,
        in_reply_to: Option<&str>,
        from: &str,
        subject: &str,
    ) -> String {
        let mut s = format!(
            "Message-ID: <{}>\r\nFrom: {}\r\nSubject: {}\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n",
            message_id, from, subject
        );
        if let Some(reply) = in_reply_to {
            s.push_str(&format!("In-Reply-To: <{}>\r\n", reply));
        }
        s.push_str("\r\nBody\r\n");
        s
    }

    fn make_email(path: PathBuf) -> Email {
        Email {
            message_id: "id@test".to_string(),
            reply_to: None,
            from: Address::default(),
            subject: String::new(),
            timestamp: None,
            path: RefCell::new(path),
        }
    }

    // ── is_unread ────────────────────────────────────────────────────────────

    #[test]
    fn is_unread_new_dir_always_unread() {
        assert!(make_email(PathBuf::from("/mb/new/msg")).is_unread());
    }

    #[test]
    fn is_unread_cur_with_seen_flag() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,S")).is_unread());
    }

    #[test]
    fn is_unread_cur_without_seen_flag() {
        assert!(make_email(PathBuf::from("/mb/cur/msg:2,")).is_unread());
    }

    #[test]
    fn is_unread_cur_multiple_flags_including_seen() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,RS")).is_unread());
    }

    #[test]
    fn is_unread_cur_no_flags_section() {
        assert!(make_email(PathBuf::from("/mb/cur/msg")).is_unread());
    }

    #[test]
    fn is_unread_cur_unrecognized_info_format() {
        // Info section doesn't start with "2,"
        assert!(make_email(PathBuf::from("/mb/cur/msg:1,S")).is_unread());
    }

    // ── from_file ────────────────────────────────────────────────────────────

    #[test]
    fn from_file_parses_display_name() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "Alice <alice@example.com>", "Hello"),
        );
        let email = Email::from_file(&path).unwrap();
        assert_eq!(email.from.name(), "Alice");
        assert_eq!(email.from.address(), "alice@example.com");
        assert_eq!(email.from.full(), "Alice <alice@example.com>");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_falls_back_to_address_when_no_name() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "alice@example.com", "Hello"),
        );
        let email = Email::from_file(&path).unwrap();
        assert_eq!(email.from.short(), "alice@example.com");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_parses_subject_and_message_id() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("myid@test", None, "bob@example.com", "My Subject"),
        );
        let email = Email::from_file(&path).unwrap();
        assert_eq!(email.message_id, "myid@test");
        assert_eq!(email.subject, "My Subject");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_parses_in_reply_to() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email(
                "child@test",
                Some("parent@test"),
                "bob@example.com",
                "Re: Hello",
            ),
        );
        let email = Email::from_file(&path).unwrap();
        assert_eq!(email.reply_to, Some("parent@test".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_parses_timestamp() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        assert!(email.timestamp.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_missing_date_gives_none_timestamp() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            "Message-ID: <id@test>\r\nFrom: test@example.com\r\n\r\nBody\r\n",
        );
        let email = Email::from_file(&path).unwrap();
        assert_eq!(email.timestamp, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_error_on_missing_message_id() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            "From: test@example.com\r\nSubject: No ID\r\n\r\nBody\r\n",
        );
        assert!(Email::from_file(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_error_on_nonexistent_file() {
        assert!(Email::from_file(&PathBuf::from("/nonexistent/path/file")).is_err());
    }

    // ── mark / has_mark / clear_mark (Seen / Unseen) ─────────────────────────

    #[test]
    fn mark_seen_moves_from_new_to_cur() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Seen);
        assert!(email.path().components().any(|c| c.as_os_str() == "cur"));
        assert!(email.path().to_str().unwrap().contains(":2,S"));
        assert!(!email.is_unread());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_seen_adds_flag_in_cur() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Seen);
        assert!(email.path().to_str().unwrap().contains(":2,S"));
        assert!(!email.is_unread());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_seen_already_seen_is_noop() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        let original = email.path().clone();
        email.mark(Flag::Seen);
        assert_eq!(*email.path(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_seen_inserts_in_alphabetical_order() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,RT");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Seen);
        let p = email.path();
        let name = p.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "msg1:2,RST");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_seen_cur_no_flags_appends() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Seen);
        assert!(email.path().to_str().unwrap().ends_with(":2,S"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_unseen_in_new_is_noop() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Unseen);
        assert!(email.is_unread());
        assert_eq!(email.path().file_name().unwrap(), "msg1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_unseen_removes_seen_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Unseen);
        assert!(email.is_unread());
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_unseen_removes_s_from_multiple_flags() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,RST");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Unseen);
        assert!(email.is_unread());
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,RT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_unseen_already_unread_is_noop() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        let original = email.path().clone();
        email.mark(Flag::Unseen);
        assert_eq!(*email.path(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seen_unseen_roundtrip() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Seen);
        assert!(!email.is_unread());
        email.mark(Flag::Unseen);
        assert!(email.is_unread());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_mark_seen_is_same_as_mark_unseen() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.clear_mark(Flag::Seen);
        assert!(email.is_unread());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── mark / has_mark / clear_mark (Replied) ────────────────────────────────

    #[test]
    fn mark_replied_sets_r_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Replied);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,RS");
        assert!(email.has_mark(Flag::Replied));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_replied_from_new_moves_to_cur() {
        let dir = temp_dir();
        let path = dir.join("new").join("msg1");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Replied);
        assert!(email.path().components().any(|c| c.as_os_str() == "cur"));
        assert!(email.has_mark(Flag::Replied));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_mark_replied_removes_r_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,RS");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.clear_mark(Flag::Replied);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,S");
        assert!(!email.has_mark(Flag::Replied));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_mark_replied_false_when_absent() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,S")).has_mark(Flag::Replied));
    }

    // ── mark / has_mark / clear_mark (Passed) ────────────────────────────────

    #[test]
    fn mark_passed_sets_p_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Passed);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,PS");
        assert!(email.has_mark(Flag::Passed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_mark_passed_removes_p_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,PS");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.clear_mark(Flag::Passed);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,S");
        assert!(!email.has_mark(Flag::Passed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_mark_passed_false_when_absent() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,S")).has_mark(Flag::Passed));
    }

    // ── mark / clear_mark / has_mark (Flagged) ───────────────────────────────

    #[test]
    fn has_mark_flagged_true_when_present() {
        assert!(make_email(PathBuf::from("/mb/cur/msg:2,FS")).has_mark(Flag::Flagged));
    }

    #[test]
    fn has_mark_flagged_false_when_absent() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,S")).has_mark(Flag::Flagged));
    }

    #[test]
    fn mark_flagged_adds_f_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,S");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.mark(Flag::Flagged);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,FS");
        assert!(email.has_mark(Flag::Flagged));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_mark_flagged_removes_f_flag() {
        let dir = temp_dir();
        let path = dir.join("cur").join("msg1:2,FS");
        write_file(
            &path,
            &minimal_email("id@test", None, "test@example.com", "Test"),
        );
        let email = Email::from_file(&path).unwrap();
        email.clear_mark(Flag::Flagged);
        let name = email
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(name, "msg1:2,S");
        assert!(!email.has_mark(Flag::Flagged));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── has_mark (Unseen) ─────────────────────────────────────────────────────

    #[test]
    fn has_mark_unseen_new_dir_always_true() {
        assert!(make_email(PathBuf::from("/mb/new/msg")).has_mark(Flag::Unseen));
    }

    #[test]
    fn has_mark_unseen_cur_with_seen_flag() {
        assert!(!make_email(PathBuf::from("/mb/cur/msg:2,S")).has_mark(Flag::Unseen));
    }

    #[test]
    fn has_mark_unseen_cur_without_seen_flag() {
        assert!(make_email(PathBuf::from("/mb/cur/msg:2,")).has_mark(Flag::Unseen));
    }

    // ── EmailThread ──────────────────────────────────────────────────────────

    #[test]
    fn email_thread_new_stores_data_and_has_no_replies() {
        let email = make_email(PathBuf::from("/mb/new/msg"));
        let thread = EmailThread::new(email.clone());
        assert_eq!(thread.parent, email);
        assert!(thread.replies.borrow().is_empty());
    }
}
