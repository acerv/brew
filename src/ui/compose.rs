// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::address::{self, Address};
use crate::core::config;
use crate::core::thread::Email;
use anyhow::Result;

const BODY_SENTINEL: &str = "--- body ---";

/// A parsed draft ready to be sent.
pub struct Draft {
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub body: String,
}

impl Draft {
    /// Parse an edited draft string into structured fields.
    pub fn parse(edited: &str) -> Self {
        let mut to = Vec::new();
        let mut cc = Vec::new();
        let mut subject = String::new();
        let mut in_reply_to = None;

        for line in edited.lines().take_while(|l| *l != BODY_SENTINEL) {
            if let Some(val) = line.strip_prefix("To:") {
                to = address::split_addresses(val);
            } else if let Some(val) = line.strip_prefix("Cc:") {
                cc = address::split_addresses(val);
            } else if let Some(val) = line.strip_prefix("Subject:") {
                subject = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("In-Reply-To:") {
                in_reply_to = Some(val.trim().to_string());
            }
        }

        let body = edited
            .lines()
            .enumerate()
            .find(|(_, l)| *l == BODY_SENTINEL)
            .map(|(i, _)| edited.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        let body = body.trim_start_matches('\n').to_string();

        Self {
            to,
            cc,
            subject,
            in_reply_to,
            body,
        }
    }

    /// Convert this draft into a minimal RFC 2822 email string suitable for
    /// writing into a maildir folder via `Maildir::write_email`.
    pub fn to_rfc2822(&self, from: &str) -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let pid = std::process::id();
        let message_id = format!("{timestamp}.{pid}.localhost");
        let mut email = format!("Message-ID: <{message_id}>\r\n");
        email.push_str(&format!("From: {from}\r\n"));
        if !self.to.is_empty() {
            email.push_str(&format!(
                "To: {}\r\n",
                self.to
                    .iter()
                    .map(|a| a.full())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.cc.is_empty() {
            email.push_str(&format!(
                "Cc: {}\r\n",
                self.cc
                    .iter()
                    .map(|a| a.full())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(irt) = &self.in_reply_to {
            email.push_str(&format!("In-Reply-To: {irt}\r\n"));
        }
        email.push_str(&format!("Subject: {}\r\n", self.subject));
        email.push_str("MIME-Version: 1.0\r\n");
        email.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        email.push_str("\r\n");
        email.push_str(&self.body.replace('\n', "\r\n"));
        email
    }
}

/// Generate a compose draft for a new email from scratch.
pub fn compose_draft() -> String {
    let mut draft = format!("To: \nSubject: \n{BODY_SENTINEL}\n");
    if let Some(sig) = config::load_signature() {
        draft.push_str(&format!("\n--\n{sig}\n"));
    }
    draft
}

/// Convert an `Email` back into the internal draft text format so it can be
/// re-opened in the compose editor.
pub fn email_to_draft(email: &Email) -> Result<String> {
    let msg = email.to_message()?;
    let to = collect_mail_parser_addrs(msg.to())
        .iter()
        .map(|a| a.full())
        .collect::<Vec<_>>()
        .join(", ");
    let cc = collect_mail_parser_addrs(msg.cc())
        .iter()
        .map(|a| a.full())
        .collect::<Vec<_>>()
        .join(", ");
    let body = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();

    let mut draft = format!("To: {to}\n");
    if !cc.is_empty() {
        draft.push_str(&format!("Cc: {cc}\n"));
    }
    draft.push_str(&format!("Subject: {}\n", email.subject));
    if let Some(irt) = &email.reply_to {
        draft.push_str(&format!("In-Reply-To: <{irt}>\n"));
    }
    draft.push_str(&format!("{BODY_SENTINEL}\n"));
    draft.push_str(&body);
    Ok(draft)
}

/// Generate a forward draft for `email` — empty `To:`, `Fwd:` subject,
/// original body quoted with an attribution header.
pub fn forward_draft(email: &Email) -> Result<String> {
    let msg = email.to_message()?;

    let subject = if email.subject.starts_with("Fwd:") || email.subject.starts_with("fwd:") {
        email.subject.clone()
    } else {
        format!("Fwd: {}", email.subject)
    };

    let body = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();

    let mut draft = format!("To: \nSubject: {subject}\n{BODY_SENTINEL}\n");
    draft.push_str(&format!(
        "---------- Forwarded message ----------\nFrom: {}\nSubject: {}\n\n",
        email.from.full(),
        email.subject,
    ));
    for line in body.lines() {
        draft.push_str(&format!("> {line}\n"));
    }

    if let Some(sig) = config::load_signature() {
        draft.push_str(&format!("\n--\n{sig}\n"));
    }

    Ok(draft)
}

/// Extension trait for Email to generate reply drafts.
pub trait EmailReply {
    /// Generate a reply draft for this email.
    ///
    /// The draft includes:
    /// - To: set to the original sender
    /// - Cc: original To: + original Cc:, minus own_address
    /// - Subject: with "Re:" prefix
    /// - In-Reply-To: referencing this message
    /// - Body: optionally quoted with ">" prefix
    fn reply_draft(&self, quote: bool, own_address: &str) -> Result<String>;
}

impl EmailReply for Email {
    fn reply_draft(&self, quote: bool, own_address: &str) -> Result<String> {
        let msg = self.to_message()?;

        let reply_subject = if self.subject.starts_with("Re:") || self.subject.starts_with("re:") {
            self.subject.clone()
        } else {
            format!("Re: {}", self.subject)
        };

        // Cc = original To + original Cc, minus own address
        let mut cc_addrs: Vec<Address> = collect_mail_parser_addrs(msg.to())
            .into_iter()
            .chain(collect_mail_parser_addrs(msg.cc()))
            .filter(|a| a.address().to_lowercase() != own_address.to_lowercase())
            .collect();
        // Deduplicate by address
        let mut seen = std::collections::HashSet::new();
        cc_addrs.retain(|a| seen.insert(a.address().to_lowercase()));
        let cc = cc_addrs
            .iter()
            .map(|a| a.full())
            .collect::<Vec<_>>()
            .join(", ");

        let mut draft = format!(
            "To: {}\nSubject: {}\nIn-Reply-To: <{}>\n",
            self.from.full(),
            reply_subject,
            self.message_id
        );

        if !cc.is_empty() {
            draft.push_str(&format!("Cc: {}\n", cc));
        }

        draft.push_str(&format!("{}\n", BODY_SENTINEL));

        if quote {
            let body = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
            for line in body.lines() {
                draft.push_str(&format!("> {}\n", line));
            }
        }

        if let Some(sig) = config::load_signature() {
            draft.push_str(&format!("\n--\n{}\n", sig));
        }

        Ok(draft)
    }
}

/// Collect mail_parser addresses into a Vec<Address>.
fn collect_mail_parser_addrs(addrs: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    addrs
        .map(|a| {
            a.iter()
                .map(|addr| {
                    Address::new(
                        addr.name().unwrap_or_default(),
                        addr.address().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_draft ─────────────────────────────────────────────────────────

    #[test]
    fn parse_draft_extracts_all_fields() {
        let text = "To: alice@x.com\nCc: bob@x.com\nSubject: Hello\nIn-Reply-To: <123>\n--- body ---\nHi there";
        let d = Draft::parse(text);
        assert_eq!(d.to.len(), 1);
        assert_eq!(d.to[0].address(), "alice@x.com");
        assert_eq!(d.cc.len(), 1);
        assert_eq!(d.cc[0].address(), "bob@x.com");
        assert_eq!(d.subject, "Hello");
        assert_eq!(d.in_reply_to.as_deref(), Some("<123>"));
        assert_eq!(d.body, "Hi there");
    }

    #[test]
    fn parse_draft_multiple_to() {
        let text = "To: a@x.com, b@x.com\nSubject: Hi\n--- body ---\n";
        let d = Draft::parse(text);
        assert_eq!(d.to.len(), 2);
    }

    #[test]
    fn parse_draft_no_in_reply_to() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\nBody";
        let d = Draft::parse(text);
        assert!(d.in_reply_to.is_none());
    }

    #[test]
    fn parse_draft_empty_body() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\n";
        let d = Draft::parse(text);
        assert!(d.body.is_empty());
    }

    #[test]
    fn parse_draft_no_sentinel() {
        let text = "To: a@x.com\nSubject: Hi\n";
        let d = Draft::parse(text);
        assert!(d.body.is_empty());
    }

    #[test]
    fn parse_draft_multiline_body() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\nLine 1\nLine 2\nLine 3";
        let d = Draft::parse(text);
        assert_eq!(d.body, "Line 1\nLine 2\nLine 3");
    }

    // ── compose_draft ───────────────────────────────────────────────────────

    #[test]
    fn compose_draft_has_required_headers() {
        let draft = compose_draft();
        assert!(draft.contains("To: "));
        assert!(draft.contains("Subject: "));
        assert!(draft.contains(BODY_SENTINEL));
    }

    #[test]
    fn compose_draft_roundtrips() {
        let draft = compose_draft();
        let d = Draft::parse(&draft);
        assert!(d.to.is_empty());
        assert!(d.subject.is_empty());
        assert!(d.in_reply_to.is_none());
    }

    // ── draft_to_rfc2822 ────────────────────────────────────────────────────

    #[test]
    fn draft_to_rfc2822_contains_message_id() {
        let text = "To: alice@x.com\nSubject: Hi\n--- body ---\nHello";
        let rfc = Draft::parse(text).to_rfc2822("me@x.com");
        assert!(rfc.contains("Message-ID:"), "got: {rfc}");
    }

    #[test]
    fn draft_to_rfc2822_contains_required_headers() {
        let text = "To: alice@x.com\nCc: bob@x.com\nSubject: Hi\n--- body ---\nHello";
        let rfc = Draft::parse(text).to_rfc2822("Me <me@x.com>");
        assert!(rfc.contains("From: Me <me@x.com>"), "got: {rfc}");
        assert!(rfc.contains("To: alice@x.com"), "got: {rfc}");
        assert!(rfc.contains("Cc: bob@x.com"), "got: {rfc}");
        assert!(rfc.contains("Subject: Hi"), "got: {rfc}");
    }

    #[test]
    fn draft_to_rfc2822_contains_body() {
        let text = "To: alice@x.com\nSubject: Hi\n--- body ---\nHello world";
        let rfc = Draft::parse(text).to_rfc2822("me@x.com");
        assert!(rfc.contains("Hello world"), "got: {rfc}");
    }

    #[test]
    fn draft_to_rfc2822_includes_in_reply_to() {
        let text = "To: alice@x.com\nSubject: Re: Hi\nIn-Reply-To: <abc@x.com>\n--- body ---\n";
        let rfc = Draft::parse(text).to_rfc2822("me@x.com");
        assert!(rfc.contains("In-Reply-To: <abc@x.com>"), "got: {rfc}");
    }

    // ── email_to_draft ──────────────────────────────────────────────────────

    fn write_tmp_email(content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "brew-compose-test-{}-{}.eml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn email_to_draft_restores_to_and_subject() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Hello\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody text\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email_to_draft(&email).unwrap();
        assert!(draft.contains("To: bob@x.com"), "got: {draft}");
        assert!(draft.contains("Subject: Hello"), "got: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn email_to_draft_restores_cc() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nCc: carol@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email_to_draft(&email).unwrap();
        assert!(draft.contains("Cc: carol@x.com"), "got: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn email_to_draft_restores_body() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nHello world\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email_to_draft(&email).unwrap();
        assert!(draft.contains("Hello world"), "got: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn email_to_draft_roundtrips_via_rfc2822() {
        let original = "To: alice@x.com\nSubject: Test\n--- body ---\nHi there";
        let rfc = Draft::parse(original).to_rfc2822("me@x.com");
        let path = write_tmp_email(&rfc);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email_to_draft(&email).unwrap();
        assert!(draft.contains("To: alice@x.com"), "got: {draft}");
        assert!(draft.contains("Subject: Test"), "got: {draft}");
        assert!(draft.contains("Hi there"), "got: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    // ── reply_draft Cc behaviour ─────────────────────────────────────────────

    #[test]
    fn reply_draft_cc_includes_original_to() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: me@x.com, bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email.reply_draft(false, "me@x.com").unwrap();
        assert!(
            draft.contains("bob@x.com"),
            "original To recipient should be in Cc: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reply_draft_cc_excludes_own_address() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: me@x.com, bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email.reply_draft(false, "me@x.com").unwrap();
        let cc_line = draft.lines().find(|l| l.starts_with("Cc:")).unwrap_or("");
        assert!(
            !cc_line.contains("me@x.com"),
            "own address must not appear in Cc: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reply_draft_cc_merges_original_cc() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nCc: carol@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email.reply_draft(false, "me@x.com").unwrap();
        assert!(
            draft.contains("bob@x.com"),
            "original To should be in Cc: {draft}"
        );
        assert!(
            draft.contains("carol@x.com"),
            "original Cc should be in Cc: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reply_draft_deduplicates_cc() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nCc: bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = email.reply_draft(false, "me@x.com").unwrap();
        let cc_line = draft.lines().find(|l| l.starts_with("Cc:")).unwrap_or("");
        let count = cc_line.matches("bob@x.com").count();
        assert_eq!(count, 1, "bob@x.com must appear only once in Cc: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    // ── forward_draft ────────────────────────────────────────────────────────

    #[test]
    fn forward_draft_to_is_empty() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Hello\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody text\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = forward_draft(&email).unwrap();
        let to_line = draft.lines().find(|l| l.starts_with("To:")).unwrap_or("");
        assert_eq!(to_line, "To: ", "To: must be empty: {draft}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forward_draft_adds_fwd_prefix() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Hello\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = forward_draft(&email).unwrap();
        assert!(
            draft.contains("Subject: Fwd: Hello"),
            "subject must have Fwd: prefix: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forward_draft_does_not_double_fwd_prefix() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Fwd: Hello\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = forward_draft(&email).unwrap();
        assert!(
            !draft.contains("Fwd: Fwd:"),
            "subject must not double Fwd: prefix: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forward_draft_includes_original_body_quoted() {
        let content = "Message-ID: <id@test>\r\nFrom: alice@x.com\r\nTo: bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nOriginal body\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = forward_draft(&email).unwrap();
        assert!(
            draft.contains("> Original body"),
            "body must be quoted: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forward_draft_includes_attribution_header() {
        let content = "Message-ID: <id@test>\r\nFrom: Alice <alice@x.com>\r\nTo: bob@x.com\r\nSubject: Hi\r\nDate: Mon, 01 Jan 2024 00:00:00 +0000\r\n\r\nBody\r\n";
        let path = write_tmp_email(content);
        let email = crate::core::thread::Email::from_file(&path).unwrap();
        let draft = forward_draft(&email).unwrap();
        assert!(
            draft.contains("Forwarded message"),
            "draft must contain attribution header: {draft}"
        );
        assert!(
            draft.contains("alice@x.com"),
            "attribution must include original sender: {draft}"
        );
        let _ = std::fs::remove_file(&path);
    }
}
