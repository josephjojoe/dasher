//! `dasher-bench micro`: CPU per operation for small fixed loops of
//! language operations and DOM calls.
//!
//! The loops live in `crates/bench/micro/{language,dom}.js`. Each benchmark
//! is one page load: the suite's page (empty for `language`, about 9,000
//! nodes for `dom`) with the suite's script, told by its query string which
//! loop to run. The same page with no loop to run is the baseline. The
//! benchmark's cost is the median CPU time of its loads less the median of
//! the baseline's, divided by its iterations. A loop that takes under
//! [`MIN_LOOP`] is first given more iterations, in proportion. CPU time is the calling
//! thread's, which is where dasher runs the page, so the in-process fixture
//! server does not count.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use dasher::{Browser, GotoOptions};
use dasher_test_server::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

use crate::sys;

const USAGE: &str = "\
usage: dasher-bench micro [options]

Times small fixed loops of language operations and DOM calls inside dasher
and prints the CPU time per operation, with the cost of loading the same
page without the loop subtracted, as the median of several runs.

options:
  --runs N          loads of each benchmark and of each baseline (default 5)
  --filter TEXT     only benchmarks whose name contains TEXT, or one suite
                    (`language` or `dom`)
  --json PATH       also write the results as JSON
  -h, --help        show this help";

/// A suite: its script, and the document its loops run in.
struct Suite {
    name: &'static str,
    script: &'static str,
}

const SUITES: &[Suite] = &[
    Suite {
        name: "language",
        script: include_str!("../micro/language.js"),
    },
    Suite {
        name: "dom",
        script: include_str!("../micro/dom.js"),
    },
];

/// Runs the loop named by the query string (`?name` or `?name&iterations`),
/// if any, and sets `benchDone` once it has finished (after its promise
/// settles, for async loops).
const RUNNER: &str = r#"
var benchDone = false;
(function () {
  var q = location.search.slice(1).split("&");
  var name = decodeURIComponent(q[0]);
  if (!name) { benchDone = true; return; }
  var b = MICRO.find(function (b) { return b.name === name; });
  var r = b.fn(q[1] ? Number(q[1]) : b.n);
  if (r && typeof r.then === "function") r.then(function () { benchDone = true; });
  else benchDone = true;
})();
"#;

/// Loops that take less CPU than this get more iterations, so that the
/// baseline's jitter stays small beside them as dasher gets faster.
const MIN_LOOP: Duration = Duration::from_millis(100);

/// Cards in the DOM suite's page; see the top of `micro/dom.js`.
const CARDS: usize = 500;

struct Options {
    runs: usize,
    filter: Option<String>,
    json: Option<PathBuf>,
}

struct Benchmark {
    suite: &'static str,
    name: String,
    iterations: u64,
    /// Median CPU time of a load with the loop, and the baseline's.
    loaded: Duration,
    baseline: Duration,
}

impl Benchmark {
    /// CPU time of the loop alone.
    fn cpu(&self) -> Duration {
        self.loaded.saturating_sub(self.baseline)
    }

    fn ns_per_op(&self) -> f64 {
        self.cpu().as_secs_f64() * 1e9 / self.iterations as f64
    }
}

pub fn main(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("dasher-bench micro: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dasher-bench micro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        runs: 5,
        filter: None,
        json: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--runs" => {
                options.runs = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--runs needs a positive number")?;
            }
            "--filter" => options.filter = Some(value()?),
            "--json" => options.json = Some(value()?.into()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Some(options))
}

fn run(options: &Options) -> Result<(), String> {
    let server = server().map_err(|e| format!("cannot start the fixture server: {e}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let browser = Browser::new().map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    let mut dom_nodes = 0;
    runtime.block_on(async {
        for suite in SUITES {
            let page_url = server.url(&format!("/micro/{}.html", suite.name));
            // Warm up, and read the suite's list of loops.
            let mut page = browser.new_page();
            load(&mut page, &page_url).await?;
            if suite.name == "dom" {
                dom_nodes = page.memory_usage().dom_nodes;
            }
            let list: Vec<(String, u64)> = page
                .evaluate_as("MICRO.map(b => [b.name, b.n])")
                .await
                .map_err(|e| format!("{}: {e}", suite.name))?;
            drop(page);

            let list: Vec<_> = list
                .into_iter()
                .filter(|(name, _)| match &options.filter {
                    None => true,
                    Some(f) => f == suite.name || name.contains(f.as_str()),
                })
                .collect();
            if list.is_empty() {
                continue;
            }
            eprintln!(
                "dasher-bench micro: {} ({} benchmarks, {} runs)",
                suite.name,
                list.len(),
                options.runs
            );
            // Calibrate: give loops that run under MIN_LOOP more iterations.
            let calibration = timed_load(&browser, &page_url).await?;
            let mut list = list;
            for (name, iterations) in &mut list {
                let url = format!("{page_url}?{}", encode(name));
                let cpu = timed_load(&browser, &url)
                    .await?
                    .saturating_sub(calibration);
                if cpu < MIN_LOOP {
                    let floor = Duration::from_micros(500);
                    let factor = MIN_LOOP.as_secs_f64() / cpu.max(floor).as_secs_f64();
                    *iterations = (*iterations as f64 * factor).ceil() as u64;
                }
            }
            let urls: Vec<String> = list
                .iter()
                .map(|(name, n)| format!("{page_url}?{}&{n}", encode(name)))
                .collect();
            // Interleave the runs, so that drift hits every benchmark alike.
            let mut baseline = Vec::new();
            let mut loaded = vec![Vec::new(); list.len()];
            for _ in 0..options.runs {
                baseline.push(timed_load(&browser, &page_url).await?);
                for (times, url) in loaded.iter_mut().zip(&urls) {
                    times.push(timed_load(&browser, url).await?);
                }
            }
            let baseline = median(baseline);
            for ((name, iterations), times) in list.into_iter().zip(loaded) {
                results.push(Benchmark {
                    suite: suite.name,
                    name,
                    iterations,
                    loaded: median(times),
                    baseline,
                });
            }
        }
        Ok::<_, String>(())
    })?;

    println!("{}", markdown(&results, options.runs, dom_nodes));
    if let Some(path) = &options.json {
        let text = serde_json::to_string_pretty(&to_json(&results, options.runs, dom_nodes))
            .unwrap_or_default();
        std::fs::write(path, text + "\n")
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Loads `url` in a new page and returns the thread's CPU time for it.
async fn timed_load(browser: &Browser, url: &str) -> Result<Duration, String> {
    let mut page = browser.new_page();
    let before = sys::thread_cpu();
    load(&mut page, url).await?;
    let cpu = sys::thread_cpu().saturating_sub(before);
    let done = page
        .evaluate("benchDone === true")
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if done != Value::Bool(true) {
        return Err(format!("{url}: the loop did not finish"));
    }
    Ok(cpu)
}

/// Loads `url` up to its `load` event and fails if a script threw.
async fn load(page: &mut dasher::Page, url: &str) -> Result<(), String> {
    page.goto_with(url, &GotoOptions::load())
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    match page.errors().first() {
        Some(error) => Err(format!("{url}: {}", error.message)),
        None => Ok(()),
    }
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// Percent-encodes a benchmark name for a query string.
fn encode(name: &str) -> String {
    name.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}

fn server() -> std::io::Result<FixtureServer> {
    let mut builder = FixtureServer::builder();
    for suite in SUITES {
        let body = match suite.name {
            "dom" => dom_page(),
            _ => String::new(),
        };
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{name}</title>{style}</head>\
             <body>{body}<script src=\"/micro/{name}.js\"></script><script>{RUNNER}</script>\
             </body></html>",
            name = suite.name,
            style = if suite.name == "dom" { DOM_STYLE } else { "" },
        );
        let script = suite.script;
        builder = builder
            .route(&format!("/micro/{}.html", suite.name), move |_| {
                FixtureResponse::html(html.clone())
            })
            .route(&format!("/micro/{}.js", suite.name), move |_| {
                FixtureResponse::ok("text/javascript; charset=utf-8", script)
            });
    }
    builder.start()
}

const DOM_STYLE: &str = "<style>.card{margin:4px;padding:2px}.text{color:#333}\
                         #target{display:block;width:100px}</style>";

/// The DOM suite's document, with no whitespace between tags so that its
/// node count is exact: about 9,000 nodes.
fn dom_page() -> String {
    let mut html = String::from("<main>");
    for i in 0..CARDS {
        if i == CARDS / 2 {
            html.push_str(
                "<div id=\"target\" class=\"a b c\" data-x=\"1\" style=\"color: red\" \
                 title=\"t\"><span>s</span><p id=\"para\">Hello world</p></div>",
            );
        }
        let _ = write!(
            html,
            "<section class=\"card\" id=\"c{i}\" data-x=\"{i}\"><h2>Card {i}</h2>\
             <p class=\"text\">Text of card {i}</p><ul>"
        );
        for j in 0..4 {
            let _ = write!(html, "<li><a href=\"/c/{i}/{j}\">Link {j}</a></li>");
        }
        html.push_str("</ul></section>");
    }
    html.push_str("<div id=\"last\"></div></main>");
    html
}

fn format_ns(ns: f64) -> String {
    if ns >= 100.0 {
        format!("{ns:.0}")
    } else {
        format!("{ns:.1}")
    }
}

fn markdown(results: &[Benchmark], runs: usize, dom_nodes: usize) -> String {
    let mut out = format!(
        "## dasher micro-benchmarks\n\nCPU per operation, median of {runs} runs, \
         page load without the loop subtracted. The DOM page has {dom_nodes} nodes.\n\n\
         | suite | benchmark | iterations | ns per op | loop CPU |\n\
         | --- | --- | ---: | ---: | ---: |\n"
    );
    for b in results {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.1} ms |",
            b.suite,
            b.name,
            b.iterations,
            format_ns(b.ns_per_op()),
            b.cpu().as_secs_f64() * 1000.0,
        );
    }
    out
}

fn to_json(results: &[Benchmark], runs: usize, dom_nodes: usize) -> Value {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    json!({
        "runs": runs,
        "dom_nodes": dom_nodes,
        "benchmarks": results.iter().map(|b| json!({
            "suite": b.suite,
            "name": b.name,
            "iterations": b.iterations,
            "ns_per_op": b.ns_per_op(),
            "loop_cpu_ms": ms(b.cpu()),
            "load_cpu_ms": ms(b.loaded),
            "baseline_cpu_ms": ms(b.baseline),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_encoded_for_a_query_string() {
        assert_eq!(
            encode("querySelectorAll(#id)"),
            "querySelectorAll%28%23id%29"
        );
        assert_eq!(encode("map/filter/reduce"), "map%2Ffilter%2Freduce");
        assert_eq!(encode("a+b c"), "a%2Bb%20c");
    }

    /// Every loop runs, once, without throwing: a change to dasher that
    /// breaks one fails here rather than in someone's benchmark run.
    #[test]
    fn every_loop_runs() {
        let server = server().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let browser = Browser::new().unwrap();
        runtime.block_on(async {
            for suite in SUITES {
                let page_url = server.url(&format!("/micro/{}.html", suite.name));
                let mut page = browser.new_page();
                load(&mut page, &page_url).await.unwrap();
                let names: Vec<String> = page.evaluate_as("MICRO.map(b => b.name)").await.unwrap();
                assert!(names.len() > 20, "{}: {names:?}", suite.name);
                for name in names {
                    let url = format!("{page_url}?{}&1", encode(&name));
                    timed_load(&browser, &url).await.unwrap();
                }
            }
        });
    }

    #[test]
    fn the_dom_page_has_its_named_elements() {
        let page = dom_page();
        for id in ["target", "para", "last"] {
            assert_eq!(page.matches(&format!("id=\"{id}\"")).count(), 1, "{id}");
        }
        assert_eq!(page.matches("<section").count(), CARDS);
    }
}
