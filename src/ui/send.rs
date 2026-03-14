// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use super::app::draw_list_popup;
use crate::core::address::Address;
use crate::core::config::Smtp;
use anyhow::anyhow;
use crossterm::event::{self, Event, KeyCode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;

#[derive(PartialEq)]
pub enum SendAction {
    Sent,
    Discard,
    SaveDraft,
    GoBack,
}

pub fn confirm_send(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    has_drafts: bool,
) -> anyhow::Result<SendAction> {
    let labels: &[&str] = if has_drafts {
        &["Send Email", "Save as draft", "Discard"]
    } else {
        &["Send Email", "Discard"]
    };

    let to_action = |idx: usize| match labels[idx] {
        "Send Email" => SendAction::Sent,
        "Save as draft" => SendAction::SaveDraft,
        _ => SendAction::Discard,
    };

    let owned: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    let mut selected: usize = 0;

    loop {
        let sel = selected;
        terminal.draw(|frame| {
            draw_list_popup(frame, " Send message? ", &owned, sel);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(labels.len().saturating_sub(1));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Enter => {
                    return Ok(to_action(selected));
                }
                KeyCode::Esc => {
                    return Ok(SendAction::GoBack);
                }
                _ => {}
            }
        }
    }
}

pub fn send_message(
    smtp: &Smtp,
    to: &[Address],
    cc: &[Address],
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> anyhow::Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let from = Address::new(smtp.name.as_deref().unwrap_or(""), &smtp.username);
    let mut builder = Message::builder().from(
        from.full()
            .parse()
            .map_err(|e| anyhow!("invalid From address: {e}"))?,
    );
    for addr in to {
        builder = builder.to(addr
            .full()
            .parse()
            .map_err(|e| anyhow!("invalid To address {addr}: {e}"))?);
    }
    for addr in cc {
        builder = builder.cc(addr
            .full()
            .parse()
            .map_err(|e| anyhow!("invalid Cc address {addr}: {e}"))?);
    }
    if let Some(irt) = in_reply_to {
        builder = builder.in_reply_to(irt.to_string());
    }
    let email = builder
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| anyhow!("failed to build message: {e}"))?;

    let creds = Credentials::new(smtp.username.clone(), smtp.password.clone());
    let transport = SmtpTransport::starttls_relay(&smtp.host)
        .map_err(|e| anyhow!("SMTP relay error: {e}"))?
        .port(smtp.port)
        .credentials(creds)
        .build();

    transport
        .send(&email)
        .map_err(|e| anyhow!("SMTP send error: {e}"))?;

    Ok(())
}
