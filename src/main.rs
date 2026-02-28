mod cache;
mod config;
mod read;
mod ui;

use config::Config;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let mailbox_cfgs: Vec<&config::Mailbox> = cfg.mailboxes.iter().collect();
    ui::run(&mailbox_cfgs, &cfg.smtp)
}
