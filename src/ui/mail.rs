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

/// Build a reply draft, open vim, ask for confirmation, then send via SMTP.
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
    let self_addr = smtp.username.as_str();
    let mut to_addrs: Vec<String> = bare_address(&tab.from)
        .map(str::to_string)
        .into_iter()
        .collect();
    for a in split_addresses(&tab.to, self_addr) {
        if !to_addrs.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            to_addrs.push(a);
        }
    }
    let cc_addrs = split_addresses(&tab.cc, self_addr);

    let subject = if tab.title.starts_with("Re:") || tab.title.starts_with("re:") {
        tab.title.clone()
    } else {
        format!("Re: {}", tab.title)
    };

    let mut header_count = 2usize; // To + Subject
    let mut draft = String::new();
    draft.push_str(&format!("To: {}\n", to_addrs.join(", ")));
    draft.push_str(&format!("Subject: {}\n", subject));
    if !cc_addrs.is_empty() {
        draft.push_str(&format!("Cc: {}\n", cc_addrs.join(", ")));
        header_count += 1;
    }
    if let Some(ref mid) = tab.message_id {
        draft.push_str(&format!("In-Reply-To: <{}>\n", mid));
        header_count += 1;
    }
    draft.push_str("--\n");
    draft.push('\n');
    if quote {
        for line in tab.body.lines() {
            draft.push_str(&format!("> {}\n", line));
        }
    }
    if let Some(sig) = signature {
        draft.push_str("--\n");
        draft.push_str(sig.trim_end());
        draft.push('\n');
    }

    let tmp_path = std::env::temp_dir().join(format!("mail-reply-{}.eml", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(draft.as_bytes())?;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let reply_line = header_count + 2;
    Command::new(&editor)
        .arg(format!("+{}", reply_line))
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

    let body = edited
        .lines()
        .enumerate()
        .find(|(_, l)| *l == "--")
        .map(|(i, _)| edited.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|| edited.clone());
    let body = body.trim_start_matches('\n');

    let to_refs: Vec<&str> = to_addrs.iter().map(|s| s.as_str()).collect();
    let cc_refs: Vec<&str> = cc_addrs.iter().map(|s| s.as_str()).collect();
    if let Err(e) = send_message(smtp, &to_refs, &cc_refs, &subject, body) {
        show_error(&e.to_string(), terminal)?;
    }

    Ok(())
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
    let self_addr = smtp.username.as_str();
    let mut to_addrs: Vec<String> = bare_address(&tab.from)
        .map(str::to_string)
        .into_iter()
        .collect();
    for a in split_addresses(&tab.to, self_addr) {
        if !to_addrs.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            to_addrs.push(a);
        }
    }
    let cc_addrs = split_addresses(&tab.cc, self_addr);

    let subject = if tab.title.starts_with("Re:") || tab.title.starts_with("re:") {
        tab.title.clone()
    } else {
        format!("Re: {}", tab.title)
    };

    let mut header_count = 2usize;
    let mut draft = String::new();
    draft.push_str(&format!("To: {}\n", to_addrs.join(", ")));
    draft.push_str(&format!("Subject: {}\n", subject));
    if !cc_addrs.is_empty() {
        draft.push_str(&format!("Cc: {}\n", cc_addrs.join(", ")));
        header_count += 1;
    }
    if let Some(ref mid) = tab.message_id {
        draft.push_str(&format!("In-Reply-To: <{}>\n", mid));
        header_count += 1;
    }
    draft.push_str("--\n");
    draft.push_str(thanks.trim_end());
    draft.push('\n');
    if let Some(sig) = signature {
        draft.push_str("--\n");
        draft.push_str(sig.trim_end());
        draft.push('\n');
    }

    let tmp_path = std::env::temp_dir().join(format!("mail-thanks-{}.eml", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(draft.as_bytes())?;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

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

    let body = edited
        .lines()
        .enumerate()
        .find(|(_, l)| *l == "--")
        .map(|(i, _)| edited.lines().skip(i + 1).collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|| edited.clone());
    let body = body.trim_start_matches('\n');

    let to_refs: Vec<&str> = to_addrs.iter().map(|s| s.as_str()).collect();
    let cc_refs: Vec<&str> = cc_addrs.iter().map(|s| s.as_str()).collect();
    if let Err(e) = send_message(smtp, &to_refs, &cc_refs, &subject, body) {
        show_error(&e.to_string(), terminal)?;
    }

    Ok(())
}

/// Send a message via SMTP using lettre.
///
/// `to` must be non-empty. `cc` may be empty.
fn send_message(smtp: &Smtp, to: &[&str], cc: &[&str], subject: &str, body: &str) -> Result<()> {
    use anyhow::anyhow;

    let mut builder = Message::builder().from(
        smtp.username
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
        let lines_clone = wrapped.clone();
        terminal.draw(move |frame| {
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

            let mut text: Vec<Line> = lines_clone
                .iter()
                .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(Color::Red))))
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

/// Split a comma-separated address list (as produced by `format_addr_list`)
/// into individual bare `user@host` addresses, excluding `self_addr`.
fn split_addresses(list: &str, self_addr: &str) -> Vec<String> {
    list.split(',')
        .filter_map(|s| bare_address(s.trim()))
        .map(str::to_string)
        .filter(|a| !a.eq_ignore_ascii_case(self_addr))
        .collect()
}
