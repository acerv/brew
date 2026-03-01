// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
mod core;
mod ui;

use core::config::Config;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let mailbox_cfgs: Vec<&core::config::Mailbox> = cfg.mailboxes.iter().collect();
    ui::run(&mailbox_cfgs, &cfg.smtp, cfg.sync.as_ref())
}
