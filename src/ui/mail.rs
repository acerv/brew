// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use crate::core::config::Smtp;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::io::{self, Write};
use std::process::Command;

use super::tab::EmailTab;

// ── reply ─────────────────────────────────────────────────────────────────────

/// The sentinel line that separates draft headers from body in the temp file.
/// Chosen to be distinctive enough that it cannot plausibly appear in a
/// real header value, avoiding the fragility of a bare `--`.
const BODY_SENTINEL: &str = "--- body ---";

/// Build a reply draft, open the editor, ask for confirmation, then send.
///
/// Reply-all: To = original sender + original To (minus self);
///            Cc = original Cc (minus self).
/// `quote` — when true the original body is included quoted with "> ".
/// TUI is suspended while the editor has the terminal.
pub fn reply(
    tab: &EmailTab,
    quote: bool,
    signature: Option<&str>,
    smtp: &Smtp,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let body_content = if quote {
        let mut s = String::new();
        for line in tab.body.lines() {
            s.push_str(&format!("> {}\n", line));
        }
        s
    } else {
        String::new()
    };
    compose_and_send(tab, &body_content, "reply", signature, smtp, terminal)
}

/// Send a pre-written thanks reply.
///
/// Opens the editor pre-filled with the thanks file content so the user can
/// review it before sending.
pub fn thanks_reply(
    tab: &EmailTab,
    thanks: &str,
    signature: Option<&str>,
    smtp: &Smtp,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let body_content = format!("{}\n", thanks.trim_end());
    compose_and_send(tab, &body_content, "thanks", signature, smtp, terminal)
}

/// Open the editor with a blank new-mail draft and send on confirmation.
///
/// The draft contains empty `To:` and `Subject:` headers followed by
/// [`BODY_SENTINEL`] and an optional signature.  The user fills everything in;
/// `To:`, `Subject:`, and `Cc:` are read back from the saved file before
/// sending.
pub fn compose_new(
    signature: Option<&str>,
    smtp: &Smtp,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let mut draft = String::new();
    draft.push_str("To: \n");
    draft.push_str("Subject: \n");
    draft.push_str(BODY_SENTINEL);
    draft.push('\n');
    if let Some(sig) = signature {
        draft.push_str("--\n");
        draft.push_str(sig.trim_end());
        draft.push('\n');
    }

    let tmp_path = std::env::temp_dir().join(format!("mail-new-{}.eml", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(draft.as_bytes())?;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    // Position cursor at the end of the To: line so the user can type right away.
    Command::new(&editor).arg("+1").arg(&tmp_path).status()?;

    let edited = std::fs::read_to_string(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;

    // Parse headers written by the user above the sentinel.
    let mut to_addrs: Vec<String> = Vec::new();
    let mut cc_addrs: Vec<String> = Vec::new();
    let mut subject = String::new();

    for line in edited.lines() {
        if line == BODY_SENTINEL {
            break;
        }
        if let Some(val) = line.strip_prefix("To:") {
            for a in val.split(',').filter_map(|s| bare_address(s.trim())) {
                to_addrs.push(a.to_string());
            }
        } else if let Some(val) = line.strip_prefix("Cc:") {
            for a in val.split(',').filter_map(|s| bare_address(s.trim())) {
                cc_addrs.push(a.to_string());
            }
        } else if let Some(val) = line.strip_prefix("Subject:") {
            subject = val.trim().to_string();
        }
    }

    // Silently abort if the user left To: empty.
    if to_addrs.is_empty() {
        return Ok(());
    }

    if !confirm_send(terminal)? {
        return Ok(());
    }

    // Extract body: everything after the BODY_SENTINEL line.
    let body = edited
        .lines()
        .enumerate()
        .find(|(_, l)| *l == BODY_SENTINEL)
        .map(|(i, _)| edited.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    let body = body.trim_start_matches('\n');

    let to_refs: Vec<&str> = to_addrs.iter().map(|s| s.as_str()).collect();
    let cc_refs: Vec<&str> = cc_addrs.iter().map(|s| s.as_str()).collect();
    if let Err(e) = send_message(smtp, &to_refs, &cc_refs, &subject, body, None) {
        show_error(&e.to_string(), terminal)?;
    }

    Ok(())
}

/// Core compose-and-send routine shared by [`reply`] and [`thanks_reply`].
///
/// Builds a draft file containing the headers, the [`BODY_SENTINEL`] line,
/// and `body_content`; opens the editor; then parses and sends on confirmation.
///
/// `tmp_suffix` is a short string embedded in the temp-file name for
/// disambiguation (e.g. `"reply"` or `"thanks"`).
fn compose_and_send(
    tab: &EmailTab,
    body_content: &str,
    tmp_suffix: &str,
    signature: Option<&str>,
    smtp: &Smtp,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let self_addr = smtp.username.as_str();

    // Build To list: keep full "Name <addr>" form for display in the draft.
    // Use bare_address only for deduplication comparisons.
    // Always include the original sender in To, even if it is ourselves.
    let mut to_addrs: Vec<String> = full_addresses(&tab.from, "");
    for addr in full_addresses(&tab.to, self_addr) {
        let bare = bare_address(&addr).unwrap_or(&addr).to_string();
        if !to_addrs
            .iter()
            .any(|x| bare_address(x).unwrap_or(x).eq_ignore_ascii_case(&bare))
        {
            to_addrs.push(addr);
        }
    }
    let cc_addrs = full_addresses(&tab.cc, self_addr);

    let subject = if tab.title.starts_with("Re:") || tab.title.starts_with("re:") {
        tab.title.clone()
    } else {
        format!("Re: {}", tab.title)
    };

    // The In-Reply-To value written into the draft (and later parsed back).
    let in_reply_to_value: Option<String> = tab.message_id.as_ref().map(|mid| format!("<{}>", mid));

    let mut header_count = 2usize; // To + Subject
    let mut draft = String::new();
    draft.push_str(&format!("To: {}\n", to_addrs.join(", ")));
    draft.push_str(&format!("Subject: {}\n", subject));
    if !cc_addrs.is_empty() {
        draft.push_str(&format!("Cc: {}\n", cc_addrs.join(", ")));
        header_count += 1;
    }
    if let Some(ref irt) = in_reply_to_value {
        draft.push_str(&format!("In-Reply-To: {}\n", irt));
        header_count += 1;
    }
    draft.push_str(BODY_SENTINEL);
    draft.push('\n');
    draft.push_str(body_content);
    if let Some(sig) = signature {
        draft.push_str("--\n");
        draft.push_str(sig.trim_end());
        draft.push('\n');
    }

    let tmp_path =
        std::env::temp_dir().join(format!("mail-{}-{}.eml", tmp_suffix, std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(draft.as_bytes())?;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    // Position the cursor at the first body line (after headers + sentinel).
    let body_line = header_count + 2;
    Command::new(&editor)
        .arg(format!("+{}", body_line))
        .arg(&tmp_path)
        .status()?;

    let edited = std::fs::read_to_string(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;

    if !confirm_send(terminal)? {
        return Ok(());
    }

    // Extract the body: everything after the BODY_SENTINEL line.
    let body = edited
        .lines()
        .enumerate()
        .find(|(_, l)| *l == BODY_SENTINEL)
        .map(|(i, _)| edited.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|| edited.clone());
    let body = body.trim_start_matches('\n');

    // Re-parse To, Cc, and In-Reply-To from the (possibly edited) header section.
    // This ensures any addresses the user typed or changed in the editor are used.
    let mut final_to: Vec<String> = Vec::new();
    let mut final_cc: Vec<String> = Vec::new();
    let mut in_reply_to_sent: Option<String> = None;
    for line in edited.lines().take_while(|l| *l != BODY_SENTINEL) {
        if let Some(val) = line.strip_prefix("To:") {
            final_to = full_addresses(val, "");
        } else if let Some(val) = line.strip_prefix("Cc:") {
            final_cc = full_addresses(val, "");
        } else if let Some(val) = line.strip_prefix("In-Reply-To:") {
            in_reply_to_sent = Some(val.trim().to_string());
        }
    }

    // Abort silently if To ended up empty (user cleared it).
    if final_to.is_empty() {
        return Ok(());
    }

    let to_refs: Vec<&str> = final_to.iter().map(|s| s.as_str()).collect();
    let cc_refs: Vec<&str> = final_cc.iter().map(|s| s.as_str()).collect();
    if let Err(e) = send_message(
        smtp,
        &to_refs,
        &cc_refs,
        &subject,
        body,
        in_reply_to_sent.as_deref(),
    ) {
        show_error(&e.to_string(), terminal)?;
    }

    Ok(())
}

/// Send a message via SMTP using lettre.
///
/// `to` must be non-empty. `cc` may be empty.
/// `in_reply_to` is the raw `In-Reply-To` header value (e.g. `"<msg-id@host>"`).
fn send_message(
    smtp: &Smtp,
    to: &[&str],
    cc: &[&str],
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> Result<()> {
    use anyhow::anyhow;

    let from_str = match &smtp.name {
        Some(name) => format!("{} <{}>", name, smtp.username),
        None => smtp.username.clone(),
    };
    let mut builder = Message::builder().from(
        from_str
            .parse()
            .map_err(|e| anyhow!("invalid From address: {e}"))?,
    );
    for addr in to {
        builder = builder.to(addr
            .parse()
            .map_err(|e| anyhow!("invalid To address {addr}: {e}"))?);
    }
    for addr in cc {
        builder = builder.cc(addr
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

// ── dialogs ───────────────────────────────────────────────────────────────────

/// Show a centred error dialog with `msg` and wait for any key to dismiss.
fn show_error(
    msg: &str,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    const MAX_W: usize = 58;
    let mut wrapped: Vec<String> = Vec::new();
    for word in msg.split_whitespace() {
        match wrapped.last_mut() {
            Some(last) if last.len() + 1 + word.len() <= MAX_W => {
                last.push(' ');
                last.push_str(word);
            }
            _ => wrapped.push(word.to_string()),
        }
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    let popup_w: u16 = 68;
    let popup_h: u16 = (wrapped.len() + 4) as u16;

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let x = area.width.saturating_sub(popup_w) / 2;
            let y = area.height.saturating_sub(popup_h) / 2;
            let popup_area =
                ratatui::layout::Rect::new(x, y, popup_w.min(area.width), popup_h.min(area.height));

            use ratatui::widgets::Clear;
            frame.render_widget(Clear, popup_area);

            let block = Block::default().borders(Borders::ALL).title(Span::styled(
                " Send failed ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);

            let mut text: Vec<Line> = wrapped
                .iter()
                .map(|l| Line::from(Span::styled(l.as_str(), Style::default().fg(Color::Red))))
                .collect();
            text.push(Line::from(""));
            text.push(Line::from(Span::styled(
                "  press any key to dismiss",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(Paragraph::new(text), inner);
        })?;

        if let Event::Key(_) = event::read()? {
            return Ok(());
        }
    }
}

/// Draw a centred confirmation dialog; returns `true` on y/Enter, `false` on n/Esc.
fn confirm_send(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let popup_w: u16 = 36;
            let popup_h: u16 = 5;
            let x = area.width.saturating_sub(popup_w) / 2;
            let y = area.height.saturating_sub(popup_h) / 2;
            let popup_area =
                ratatui::layout::Rect::new(x, y, popup_w.min(area.width), popup_h.min(area.height));

            use ratatui::widgets::Clear;
            frame.render_widget(Clear, popup_area);

            let block = Block::default().borders(Borders::ALL).title(Span::styled(
                " Send reply? ",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);

            let text = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "  [ y ]",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  send    "),
                    Span::styled(
                        "[ n / Esc ]",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  cancel"),
                ]),
            ];
            frame.render_widget(Paragraph::new(text), inner);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }
}

// ── file ops ──────────────────────────────────────────────────────────────────

/// Delete the Maildir file at `path`. Errors are silently ignored.
pub fn delete_mail(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

// ── address helpers ───────────────────────────────────────────────────────────

fn bare_address(s: &str) -> Option<&str> {
    let addr = if let Some(start) = s.find('<')
        && let Some(end) = s[start..].find('>')
    {
        s[start + 1..start + end].trim()
    } else {
        s.trim()
    };
    if addr.contains('@') { Some(addr) } else { None }
}

/// Return the addresses from a comma-separated list as display strings.
///
/// Each entry is kept as `"Name <addr>"` when a display name is present,
/// or falls back to the bare `addr` otherwise.
/// Entries whose bare address matches `self_addr` are excluded.
fn full_addresses(list: &str, self_addr: &str) -> Vec<String> {
    // Split on commas outside angle brackets so "Doe, John <j@x.com>" stays intact.
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    for ch in list.chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let t = current.trim().to_string();
                if !t.is_empty() {
                    tokens.push(t);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let t = current.trim().to_string();
    if !t.is_empty() {
        tokens.push(t);
    }

    tokens
        .into_iter()
        .filter_map(|s| {
            let bare = bare_address(&s)?.to_string();
            if bare.eq_ignore_ascii_case(self_addr) {
                return None;
            }
            // Prefer "Name <addr>" if the token has a display name; otherwise bare addr.
            Some(if s.contains('<') { s } else { bare })
        })
        .collect()
}
