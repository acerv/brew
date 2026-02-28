mod cache;

use cache::MailCache;

fn main() -> anyhow::Result<()> {
    let cache = MailCache::build("/home/acer/Mail/LTP/")?;
    println!("{} threads loaded.", cache.threads.len());

    if let Some(thread) = cache.threads.first() {
        let msg = MailCache::load_mail(&thread.data)?;
        println!("First thread subject : {}", thread.data.subject);
        println!("From                 : {:?}", msg.from());
    }

    Ok(())
}
