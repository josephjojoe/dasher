//! `dasher-bench`: measures dasher's footprint on a fixture suite.
//!
//! Each measurement runs in a fresh child process (this binary, re-run with
//! an internal subcommand), so the peak RSS it reports belongs to that page
//! alone, and nothing one page leaves behind skews the next. The parent
//! serves the fixtures, collects the children's reports, prints a summary
//! table and checks the results against a budget file.
//!
//! ```text
//! cargo build --release -p dasher-cli -p dasher-bench
//! target/release/dasher-bench --budgets crates/bench/budgets.json
//! ```
//!
//! See `crates/bench/README.md` for what each number means.

mod fixtures;
mod report;
mod sys;

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use dasher::Browser;
use serde_json::{Value, json};

use crate::fixtures::{SCENARIOS, Scenario};
use crate::report::{Budgets, Results};

const USAGE: &str = "\
usage: dasher-bench [options]

Loads each fixture page in a fresh process and reports peak RSS, per-page
memory, time to settle, idle RSS, memory over sequential jobs, cold start
time and binary size.

options:
  --runs N          processes per page; times are medians, memory the maximum (default 3)
  --jobs N          sequential jobs for the memory-growth check (default 100)
  --cli PATH        the dasher binary to time and weigh (default: next to this one)
  --budgets PATH    fail if a result is over a budget in this JSON file
  --json PATH       also write the results as JSON
  --markdown PATH   also write the summary as Markdown (appends, for $GITHUB_STEP_SUMMARY)
  -h, --help        show this help";

/// Which allocator this build uses, for the report.
const ALLOCATOR: &str = "system";

struct Options {
    runs: usize,
    jobs: usize,
    cli: Option<PathBuf>,
    budgets: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("child-page") => child(|| child_page(&args[1..])),
        Some("child-jobs") => child(|| child_jobs(&args[1..])),
        _ => match parse(&args) {
            Ok(Some(options)) => run(&options),
            Ok(None) => {
                println!("{USAGE}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("dasher-bench: {message}\n\n{USAGE}");
                ExitCode::from(2)
            }
        },
    }
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options {
        runs: 3,
        jobs: 100,
        cli: None,
        budgets: None,
        json: None,
        markdown: None,
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
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("{arg} needs a positive number"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--runs" => options.runs = number(value()?)?,
            "--jobs" => options.jobs = number(value()?)?,
            "--cli" => options.cli = Some(value()?.into()),
            "--budgets" => options.budgets = Some(value()?.into()),
            "--json" => options.json = Some(value()?.into()),
            "--markdown" => options.markdown = Some(value()?.into()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Some(options))
}

// ---------------------------------------------------------------------------
// The parent: serve the fixtures, run the children, report.

fn run(options: &Options) -> ExitCode {
    let budgets = match options.budgets.as_deref().map(Budgets::load).transpose() {
        Ok(budgets) => budgets,
        Err(e) => {
            eprintln!("dasher-bench: {e}");
            return ExitCode::from(2);
        }
    };
    match measure(options) {
        Ok(results) => {
            let summary = results.markdown();
            println!("{summary}");
            let mut ok = true;
            if let Some(budgets) = &budgets {
                let over = budgets.check(&results);
                let verdict = report::verdict(&over);
                println!("{verdict}");
                ok = over.is_empty();
                if let Some(path) = &options.markdown {
                    report::append(path, &format!("{summary}\n{verdict}\n"));
                }
            } else if let Some(path) = &options.markdown {
                report::append(path, &format!("{summary}\n"));
            }
            if let Some(path) = &options.json {
                let text = serde_json::to_string_pretty(&results.json()).unwrap_or_default();
                if let Err(e) = std::fs::write(path, text + "\n") {
                    eprintln!("dasher-bench: cannot write {}: {e}", path.display());
                    ok = false;
                }
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("dasher-bench: {e}");
            ExitCode::FAILURE
        }
    }
}

fn measure(options: &Options) -> Result<Results, String> {
    let server = fixtures::server().map_err(|e| format!("cannot start the fixture server: {e}"))?;
    let mut results = Results::new(ALLOCATOR);

    for scenario in SCENARIOS {
        eprintln!("dasher-bench: {} ({} runs)", scenario.name, options.runs);
        let runs = (0..options.runs)
            .map(|_| spawn_child(&["child-page", scenario.name, &server.url(scenario.path)]))
            .collect::<Result<Vec<_>, _>>()?;
        results.add_page(scenario, &runs);
    }

    eprintln!("dasher-bench: {} sequential jobs", options.jobs);
    let urls: Vec<String> = SCENARIOS.iter().map(|s| server.url(s.path)).collect();
    let mut args = vec!["child-jobs".to_owned(), options.jobs.to_string()];
    args.extend(urls);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    results.jobs = Some(spawn_child(&args)?);

    let cli = options.cli.clone().or_else(default_cli);
    match cli.filter(|path| path.is_file()) {
        Some(cli) => {
            eprintln!("dasher-bench: cold start of {}", cli.display());
            results.binary_size = std::fs::metadata(&cli).ok().map(|m| m.len());
            let url = server.url(SCENARIOS[0].path);
            results.version_ms = Some(median_ms(options.runs.max(5), || {
                time_command(&cli, &["--version"])
            })?);
            results.cold_start_ms = Some(median_ms(options.runs.max(5), || {
                time_command(&cli, &[&url])
            })?);
        }
        None => eprintln!(
            "dasher-bench: no dasher binary found; build it with \
             `cargo build --release -p dasher-cli` or pass --cli"
        ),
    }
    Ok(results)
}

/// The `dasher` binary built alongside this one.
fn default_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(format!("dasher{}", std::env::consts::EXE_SUFFIX)))
}

/// Runs this binary with `args` and parses the JSON it prints.
fn spawn_child(args: &[&str]) -> Result<Value, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let output = Command::new(exe)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run a child process: {e}"))?;
    if !output.status.success() {
        return Err(format!("{} failed ({})", args.join(" "), output.status));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("{}: unreadable report: {e}", args.join(" ")))
}

fn time_command(program: &PathBuf, args: &[&str]) -> Result<Duration, String> {
    let started = Instant::now();
    let status = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    let elapsed = started.elapsed();
    if !status.success() {
        return Err(format!(
            "{} {} failed ({status})",
            program.display(),
            args.join(" ")
        ));
    }
    Ok(elapsed)
}

fn median_ms(runs: usize, mut f: impl FnMut() -> Result<Duration, String>) -> Result<f64, String> {
    let mut times = (0..runs)
        .map(|_| f().map(|d| d.as_secs_f64() * 1000.0))
        .collect::<Result<Vec<_>, _>>()?;
    times.sort_by(f64::total_cmp);
    Ok(times[times.len() / 2])
}

// ---------------------------------------------------------------------------
// The children: one process per measurement, reporting JSON on stdout.

fn child(f: impl FnOnce() -> Result<Value, String>) -> ExitCode {
    match f() {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dasher-bench: {e}");
            ExitCode::FAILURE
        }
    }
}

fn current_thread_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// A browser as a service would hold one: built, with the JS glue
/// compiled, before the first job.
fn ready_browser() -> Result<Browser, String> {
    let browser = Browser::new().map_err(|e| e.to_string())?;
    dasher::precompile().map_err(|e| e.to_string())?;
    Ok(browser)
}

/// `child-page <scenario> <url>`: loads one page and reports what it cost.
fn child_page(args: &[String]) -> Result<Value, String> {
    let [name, url] = args else {
        return Err("usage: child-page <scenario> <url>".into());
    };
    let scenario = fixtures::scenario(name).ok_or_else(|| format!("no scenario {name:?}"))?;
    let start_rss = sys::rss();
    let runtime = current_thread_runtime()?;
    let browser = ready_browser()?;
    let idle_rss = sys::rss();

    let (navigation, usage, loaded_rss, html_bytes) = runtime.block_on(async {
        let mut page = browser.new_page();
        let navigation = page
            .goto_with(url, &scenario.goto_options())
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        check(scenario, &page)?;
        let usage = page.memory_usage();
        let loaded_rss = sys::rss();
        let html_bytes = page.content().len();
        Ok::<_, String>((navigation, usage, loaded_rss, html_bytes))
    })?;
    let after_drop_rss = sys::rss();

    Ok(json!({
        "start_rss": start_rss,
        "idle_rss": idle_rss,
        "loaded_rss": loaded_rss,
        "after_drop_rss": after_drop_rss,
        "peak_rss": sys::peak_rss(),
        "settle_ms": navigation.elapsed.as_secs_f64() * 1000.0,
        "js_heap": usage.js_heap,
        "dom_nodes": usage.dom_nodes,
        "html_bytes": html_bytes,
        "scrolls": navigation.scroll.map_or(0, |s| s.scrolls),
    }))
}

/// `child-jobs <n> <url>...`: runs `n` jobs one after another on one
/// browser, cycling through the scenarios, each on a new page that is
/// dropped afterwards, and reports the RSS after each.
fn child_jobs(args: &[String]) -> Result<Value, String> {
    let Some((n, urls)) = args.split_first() else {
        return Err("usage: child-jobs <n> <url>...".into());
    };
    let n: usize = n.parse().map_err(|_| format!("bad job count {n:?}"))?;
    if urls.len() != SCENARIOS.len() {
        return Err("child-jobs needs one URL per scenario".into());
    }
    let runtime = current_thread_runtime()?;
    let browser = ready_browser()?;
    let idle_rss = sys::rss();
    let mut rss = Vec::with_capacity(n);
    let started = Instant::now();
    runtime.block_on(async {
        for job in 0..n {
            let i = job % SCENARIOS.len();
            let scenario = &SCENARIOS[i];
            let mut page = browser.new_page();
            page.goto_with(&urls[i], &scenario.goto_options())
                .await
                .map_err(|e| format!("job {job} ({}): {e}", scenario.name))?;
            check(scenario, &page)?;
            drop(page.content());
            drop(page);
            rss.push(sys::rss());
        }
        Ok::<_, String>(())
    })?;
    Ok(json!({
        "jobs": n,
        "idle_rss": idle_rss,
        "rss": rss,
        "peak_rss": sys::peak_rss(),
        "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
    }))
}

/// Fails unless the page rendered what it should have.
fn check(scenario: &Scenario, page: &dasher::Page) -> Result<(), String> {
    let found = page.count(scenario.selector).map_err(|e| e.to_string())?;
    if found == scenario.expected {
        Ok(())
    } else {
        Err(format!(
            "{}: expected {} elements matching {:?}, found {found}",
            scenario.name, scenario.expected, scenario.selector
        ))
    }
}
