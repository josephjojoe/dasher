//! Loading the page and printing what was asked for.

use std::io::{self, Write};
use std::process::ExitCode;

use dasher::{
    AutoScrollOptions, Browser, ClockMode, Error, GotoOptions, JsonValue, Navigation, NetworkIdle,
    Page, ScriptCache, ScriptCacheOptions, SettleOptions,
};
use dasher_serve::report::{
    console_json, error_json, error_message, level_name, scroll_json, settle_name,
};
use serde_json::{Map, json};

use crate::args::{Options, Wait};

/// Runs the page load on a single-threaded Tokio runtime, as pages need.
pub fn main(options: &Options) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("dasher: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(options))
}

async fn run(options: &Options) -> ExitCode {
    let browser = match browser(options) {
        Ok(browser) => browser,
        Err(e) => {
            eprintln!("dasher: {e}");
            return ExitCode::from(2);
        }
    };
    for cookie in &options.cookies {
        if let Err(e) = browser.set_cookie(&options.url, cookie) {
            eprintln!("dasher: {e}");
            return ExitCode::from(2);
        }
    }

    let mut page = browser.new_page();
    let navigation = match page.goto_with(&options.url, &goto_options(options)).await {
        Ok(navigation) => navigation,
        Err(e) => return fail(options, &JsonValue::Null, &e),
    };
    let result = match &options.eval {
        Some(source) => Some(page.evaluate(source).await),
        None => None,
    };

    if options.verbose {
        report_diagnostics(&page);
    }
    if navigation.timed_out {
        eprintln!(
            "dasher: timed out after {:.1}s; output is the page as far as it got",
            navigation.elapsed.as_secs_f64()
        );
    }

    let summary = summary(&page, &navigation);
    let code = match result {
        Some(Err(e)) => fail(options, &summary, &e),
        Some(Ok(value)) if options.json => print_json(&with(summary, "result", value)),
        Some(Ok(value)) => match value {
            JsonValue::String(s) => print(&s),
            other => print(&other.to_string()),
        },
        None if options.json => print_json(&with(summary, "html", page.content().into())),
        None => print(&page.content()),
    };
    // The process is about to exit and the output is written. Freeing the
    // page object by object first was 5% of a heavy page's CPU time, so
    // leave its memory for the OS to take back; unless the page is to
    // leave its compiled scripts in the cache directory, which it does
    // once it is freed.
    if options.script_cache_dir.is_some() {
        drop(page);
    } else {
        #[allow(clippy::mem_forget)]
        std::mem::forget(page);
    }
    code
}

fn browser(options: &Options) -> dasher::Result<Browser> {
    let mut builder = Browser::builder().timeout(options.timeout);
    if let Some(user_agent) = &options.user_agent {
        builder = builder.user_agent(user_agent);
    }
    if let Some((width, height)) = options.viewport {
        builder = builder.viewport(width, height);
    }
    for (name, value) in &options.headers {
        builder = builder.header(name, value);
    }
    if let Some(proxy) = &options.proxy {
        builder = builder.proxy(proxy);
    }
    for pattern in &options.block {
        builder = builder.block(pattern);
    }
    if options.realtime {
        builder = builder.clock(ClockMode::Realtime);
    }
    if let Some(visibility) = options.visibility {
        builder = builder.visibility(visibility);
    }
    if let Some(mib) = options.memory_limit {
        builder = builder.memory_limit((mib > 0).then(|| mib.saturating_mul(1024 * 1024)));
    }
    if let Some(nodes) = options.max_dom_nodes {
        builder = builder.max_dom_nodes((nodes > 0).then_some(nodes));
    }
    // One page: a cache in memory would only be filled for nothing.
    builder = builder.script_cache(options.script_cache_dir.clone().map(|dir| {
        ScriptCache::new(ScriptCacheOptions {
            dir: Some(dir),
            ..ScriptCacheOptions::default()
        })
    }));
    builder.build()
}

fn goto_options(options: &Options) -> GotoOptions {
    let quiet = options.quiet;
    let idle = |max_in_flight| {
        Some(NetworkIdle {
            max_in_flight,
            quiet,
        })
    };
    let (network_idle, dom_quiet) = match options.wait {
        Wait::Load => (None, None),
        Wait::NetworkIdle => (idle(0), None),
        Wait::NetworkIdle2 => (idle(2), None),
        Wait::DomQuiet => (None, Some(quiet)),
        Wait::Settled => (idle(0), Some(quiet)),
    };
    let settle = SettleOptions {
        network_idle,
        dom_quiet,
        selector: options.wait_for.clone(),
        predicate: options.wait_until.clone(),
        // The overall timeout bounds settling.
        timeout: options.timeout,
    };
    let waits = settle.network_idle.is_some()
        || settle.dom_quiet.is_some()
        || settle.selector.is_some()
        || settle.predicate.is_some();
    let mut goto = GotoOptions {
        settle: waits.then_some(settle),
        auto_scroll: None,
        timeout: Some(options.timeout),
    };
    if options.scroll {
        let mut scroll = AutoScrollOptions::default();
        if let Some(max) = options.max_scrolls {
            scroll.max_scrolls = max;
        }
        goto = goto.with_auto_scroll(scroll);
    }
    goto
}

/// The page-level facts the JSON output starts from.
fn summary(page: &Page, navigation: &Navigation) -> JsonValue {
    json!({
        "url": navigation.url,
        "status": navigation.status,
        "title": page.title(),
        "timed_out": navigation.timed_out,
        "settle": navigation.settle.map(settle_name),
        "scroll": navigation.scroll.as_ref().map(scroll_json),
        "elapsed_ms": u64::try_from(navigation.elapsed.as_millis()).unwrap_or(u64::MAX),
        "console": page.console().iter().map(console_json).collect::<Vec<_>>(),
        "errors": page.errors().iter().map(error_json).collect::<Vec<_>>(),
    })
}

fn with(mut object: JsonValue, key: &str, value: JsonValue) -> JsonValue {
    if let JsonValue::Object(map) = &mut object {
        map.insert(key.to_owned(), value);
    }
    object
}

/// Reports `error`: as a JSON object with an `error` field in JSON mode,
/// on stderr otherwise. Exits with status 1.
fn fail(options: &Options, summary: &JsonValue, error: &Error) -> ExitCode {
    if options.json {
        let mut object = match summary {
            JsonValue::Object(map) => map.clone(),
            _ => {
                let mut map = Map::new();
                map.insert("url".into(), options.url.clone().into());
                map
            }
        };
        object.insert("error".into(), error_message(error).into());
        if print_json(&JsonValue::Object(object)) != ExitCode::SUCCESS {
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!("dasher: {}", error_message(error));
    }
    ExitCode::FAILURE
}

fn report_diagnostics(page: &Page) {
    for message in page.console() {
        eprintln!("console.{}: {}", level_name(message.level), message.text);
    }
    for error in page.errors() {
        eprintln!("page error: {error}");
    }
}

fn print(text: &str) -> ExitCode {
    let mut out = io::stdout().lock();
    match writeln!(out, "{text}").and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        // A closed pipe (`dasher … | head`) is not worth a complaint.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dasher: cannot write output: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_json(value: &JsonValue) -> ExitCode {
    print(&value.to_string())
}
