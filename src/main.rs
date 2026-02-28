mod cache;
mod ui;

use cache::MailCache;

fn main() -> anyhow::Result<()> {
    let cache = MailCache::build("/home/acer/Mail/LTP/")?;
    ui::run(&cache)
}
