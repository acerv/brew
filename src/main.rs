mod core;
mod ui;

use core::config::Config;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let mailbox_cfgs: Vec<&core::config::Mailbox> = cfg.mailboxes.iter().collect();
    ui::run(&mailbox_cfgs, &cfg.smtp)
}
