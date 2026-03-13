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

/// Convert a draft text into a minimal RFC 2822 email string suitable for
/// writing into a maildir folder via `Maildir::write_email`.
pub fn draft_to_rfc2822(draft_text: &str, from: &str) -> String {
    let draft = parse_draft(draft_text);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let message_id = format!("{timestamp}.{pid}.localhost");
    let mut email = format!("Message-ID: <{message_id}>\r\n");
    email.push_str(&format!("From: {from}\r\n"));
    if !draft.to.is_empty() {
        email.push_str(&format!(
            "To: {}\r\n",
            draft
                .to
                .iter()
                .map(|a| a.full())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !draft.cc.is_empty() {
        email.push_str(&format!(
            "Cc: {}\r\n",
            draft
                .cc
                .iter()
                .map(|a| a.full())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(irt) = &draft.in_reply_to {
        email.push_str(&format!("In-Reply-To: {irt}\r\n"));
    }
    email.push_str(&format!("Subject: {}\r\n", draft.subject));
    email.push_str("MIME-Version: 1.0\r\n");
    email.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    email.push_str("\r\n");
    email.push_str(&draft.body.replace('\n', "\r\n"));
    email
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
