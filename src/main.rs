mod cache;
mod config;
mod ui;

use cache::MailCache;
use config::Config;

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;

    let mailboxes: Vec<(&str, MailCache)> = cfg
        .mailboxes
        .iter()
        .map(|mb| -> anyhow::Result<(&str, MailCache)> {
            let cache = MailCache::build(&mb.path)?;
            Ok((mb.label.as_str(), cache))
        })
        .collect::<anyhow::Result<_>>()?;

    ui::run(&mailboxes)
}
