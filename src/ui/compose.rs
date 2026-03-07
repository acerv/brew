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

/// Parse an edited draft string into structured fields.
pub fn parse_draft(edited: &str) -> Draft {
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

    Draft {
        to,
        cc,
        subject,
        in_reply_to,
        body,
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

/// Extension trait for Email to generate reply drafts.
pub trait EmailReply {
    /// Generate a reply draft for this email.
    ///
    /// The draft includes:
    /// - To: set to the original sender
    /// - Cc: copied from the original email (if any)
    /// - Subject: with "Re:" prefix
    /// - In-Reply-To: referencing this message
    /// - Body: optionally quoted with ">" prefix
    fn reply_draft(&self, quote: bool) -> Result<String>;
}

impl EmailReply for Email {
    fn reply_draft(&self, quote: bool) -> Result<String> {
        let msg = self.to_message()?;

        // Extract Cc using Address conversion
        let cc = format_mail_parser_addrs(msg.cc());

        let reply_subject = if self.subject.starts_with("Re:") || self.subject.starts_with("re:") {
            self.subject.clone()
        } else {
            format!("Re: {}", self.subject)
        };

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

/// Format mail_parser addresses into a comma-separated string.
fn format_mail_parser_addrs(addrs: Option<&mail_parser::Address<'_>>) -> String {
    addrs
        .map(|a| {
            a.iter()
                .map(|addr| {
                    Address::new(
                        addr.name().unwrap_or_default(),
                        addr.address().unwrap_or_default(),
                    )
                })
                .map(|a| a.full())
                .collect::<Vec<_>>()
                .join(", ")
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
        let d = parse_draft(text);
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
        let d = parse_draft(text);
        assert_eq!(d.to.len(), 2);
    }

    #[test]
    fn parse_draft_no_in_reply_to() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\nBody";
        let d = parse_draft(text);
        assert!(d.in_reply_to.is_none());
    }

    #[test]
    fn parse_draft_empty_body() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\n";
        let d = parse_draft(text);
        assert!(d.body.is_empty());
    }

    #[test]
    fn parse_draft_no_sentinel() {
        let text = "To: a@x.com\nSubject: Hi\n";
        let d = parse_draft(text);
        assert!(d.body.is_empty());
    }

    #[test]
    fn parse_draft_multiline_body() {
        let text = "To: a@x.com\nSubject: Hi\n--- body ---\nLine 1\nLine 2\nLine 3";
        let d = parse_draft(text);
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
        let d = parse_draft(&draft);
        assert!(d.to.is_empty());
        assert!(d.subject.is_empty());
        assert!(d.in_reply_to.is_none());
    }
}
