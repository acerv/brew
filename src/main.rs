// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
mod core;
mod ui;

use core::config::Config;
use ui::app::App;

fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    App::new(config)?.run()
}
