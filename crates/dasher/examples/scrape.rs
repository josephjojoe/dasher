//! Loads a page, waits for it to settle, and prints its title, links and
//! final HTML size.
//!
//! ```sh
//! cargo run -p dasher --example scrape -- https://example.com/
//! ```

use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args().nth(1).ok_or("usage: scrape <url>")?;

    let browser = dasher::Browser::builder()
        .timeout(Duration::from_secs(30))
        .block("*://*.google-analytics.com/*")
        .build()?;
    let mut page = browser.new_page();
    let navigation = page.goto(&url).await?;

    println!("{} {}", navigation.status, navigation.url);
    println!("title: {}", page.title().unwrap_or_default());
    for href in page.attributes("a[href]", "href")? {
        println!("link: {href}");
    }
    // Absolute URLs are easiest to get from JavaScript.
    let absolute: Vec<String> = page
        .evaluate_as("[...document.links].map(a => a.href)")
        .await?;
    println!(
        "{} links, {} bytes of HTML",
        absolute.len(),
        page.content().len()
    );
    for error in page.errors() {
        eprintln!("page error: {error}");
    }
    Ok(())
}
