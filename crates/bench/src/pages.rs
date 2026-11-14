//! `dasher-bench pages`: real pages, one after another in one process,
//! for what the script cache saves (see the README's "Real pages").
//!
//! Build with the `replay` feature and set `DASHER_REPLAY` to a directory
//! holding every page's recording (the recordings of several pages can be
//! merged into one directory: files are named by a hash of their URL).
//! Each line printed is one load:
//!
//! ```text
//! <name> <round> <load cpu s> <drop cpu s> <peak RSS MiB> <hits> <misses> \
//!   <hit KiB> <miss KiB> <builds> <late> <cache MiB> <elements> <text hash>
//! ```
//!
//! `load` is `goto` and reading the page out; `drop` is freeing the page,
//! which is when it remakes the cache entries it found lacking (`builds`
//! counts entries written, on a miss and then). `late` is how many
//! functions of cached scripts it still compiled on their first call. Peak RSS is the process's
//! high-water mark over the load and the drop, reset before each load
//! (Linux only), so it includes what the cache holds. Element counts and
//! a hash of the page's text (digits masked) tell whether a page came out
//! the same.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::process::ExitCode;
use std::time::Duration;

use dasher::{Browser, ScriptCache, ScriptCacheOptions, ScriptCacheStats};

const USAGE: &str = "\
usage: dasher-bench pages [options] <name=url>...

Loads each page in turn in this process and prints a line per load.

options:
  --cache off|page|shared  no script cache; a fresh one for each page; or one
                           for all pages, in the order given (default shared)
  --rounds N               loads of each page (default 1); with --cache page,
                           the first is cold and the rest warm
  --cache-mib N            the cache's memory bound (default 32)
  --timeout SECS           per load (default 60)";

pub fn main(args: &[String]) -> ExitCode {
    match parse(args) {
        Ok(Some(options)) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            runtime.block_on(run(&options));
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("dasher-bench pages: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    PerPage,
    Shared,
}

struct Options {
    mode: Mode,
    rounds: usize,
    cache_bytes: usize,
    timeout: Duration,
    pages: Vec<(String, String)>,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        mode: Mode::Shared,
        rounds: 1,
        cache_bytes: dasher::DEFAULT_SCRIPT_CACHE_BYTES,
        timeout: Duration::from_secs(60),
        pages: Vec::new(),
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        let number = |s: String| {
            s.parse::<usize>()
                .map_err(|_| format!("{arg} needs a number"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--cache" => {
                options.mode = match value()?.as_str() {
                    "off" => Mode::Off,
                    "page" => Mode::PerPage,
                    "shared" => Mode::Shared,
                    other => return Err(format!("unknown --cache {other:?}")),
                }
            }
            "--rounds" => options.rounds = number(value()?)?.max(1),
            "--cache-mib" => options.cache_bytes = number(value()?)? * 1024 * 1024,
            "--timeout" => options.timeout = Duration::from_secs(number(value()?)? as u64),
            page => {
                let (name, url) = page
                    .split_once('=')
                    .ok_or_else(|| format!("pages are name=url, not {page:?}"))?;
                options.pages.push((name.to_owned(), url.to_owned()));
            }
        }
    }
    if options.pages.is_empty() {
        return Err("no pages given".into());
    }
    Ok(Some(options))
}

async fn run(options: &Options) {
    let new_cache = || {
        ScriptCache::new(ScriptCacheOptions {
            memory_bytes: options.cache_bytes,
            ..ScriptCacheOptions::default()
        })
    };
    let shared = new_cache();
    let mut rounds = Vec::new();
    match options.mode {
        // each page `rounds` times in a row
        Mode::Off | Mode::PerPage => {
            for page in &options.pages {
                let cache = (options.mode == Mode::PerPage).then(new_cache);
                for round in 1..=options.rounds {
                    rounds.push((page, round, cache.clone()));
                }
            }
        }
        // all pages, `rounds` times over
        Mode::Shared => {
            for round in 1..=options.rounds {
                for page in &options.pages {
                    rounds.push((page, round, Some(shared.clone())));
                }
            }
        }
    }
    for ((name, url), round, cache) in rounds {
        let browser = Browser::builder()
            .timeout(options.timeout)
            .script_cache(cache.clone())
            .build()
            .expect("browser");
        let before = cache.as_ref().map(ScriptCache::stats).unwrap_or_default();
        reset_peak_rss();
        let start = crate::sys::thread_cpu();
        let mut page = browser.new_page();
        let loaded = page.goto(url).await;
        let elements: usize = page
            .evaluate_as("document.getElementsByTagName('*').length")
            .await
            .unwrap_or(0);
        let text: String = page
            .evaluate_as("document.body ? document.body.innerText : ''")
            .await
            .unwrap_or_default();
        let _html = page.content();
        let load = crate::sys::thread_cpu() - start;
        drop(page);
        let dropped = crate::sys::thread_cpu() - start - load;
        let peak = crate::sys::peak_rss();
        let after = cache.as_ref().map(ScriptCache::stats).unwrap_or_default();
        let d = |f: fn(&ScriptCacheStats) -> u64| f(&after) - f(&before);
        let mut hash = DefaultHasher::new();
        text.replace(|c: char| c.is_ascii_digit(), "0")
            .hash(&mut hash);
        if let Err(e) = loaded {
            eprintln!("{name}: {e}");
        }
        println!(
            "{name} {round} {:.3} {:.3} {:.1} {} {} {} {} {} {} {:.1} {elements} {:016x}",
            load.as_secs_f64(),
            dropped.as_secs_f64(),
            peak as f64 / (1024.0 * 1024.0),
            d(|s| s.hits),
            d(|s| s.misses),
            d(|s| s.hit_bytes) / 1024,
            d(|s| s.miss_bytes) / 1024,
            d(|s| s.builds),
            d(|s| s.compiled_late),
            after.bytes as f64 / (1024.0 * 1024.0),
            hash.finish(),
        );
    }
}

/// Resets the process's peak RSS to its RSS now (Linux).
fn reset_peak_rss() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}
