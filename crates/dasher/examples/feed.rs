//! Scrolls an infinite feed until it stops growing, then prints the text
//! of every item matching a selector.
//!
//! ```sh
//! cargo run -p dasher --example feed -- https://news.example/ "article h2"
//! ```

use std::time::Duration;

use dasher::{AutoScrollOptions, GotoOptions, SettleOptions};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(url), Some(selector)) = (args.next(), args.next()) else {
        return Err("usage: feed <url> <selector>".into());
    };

    let browser = dasher::Browser::new()?;
    let mut page = browser.new_page();
    // Wait until the first items exist, then scroll for more.
    let options = GotoOptions::settle(SettleOptions {
        selector: Some(selector.clone()),
        ..SettleOptions::default()
    })
    .with_auto_scroll(AutoScrollOptions {
        max_scrolls: 30,
        ..AutoScrollOptions::default()
    })
    .with_timeout(Duration::from_secs(90));
    let navigation = page.goto_with(&url, &options).await?;
    if let Some(scroll) = navigation.scroll {
        eprintln!(
            "scrolled {} times ({} brought more), stopped: {:?}",
            scroll.scrolls, scroll.productive_scrolls, scroll.stopped
        );
    }
    for text in page.texts(&selector)? {
        println!("{text}");
    }
    Ok(())
}
