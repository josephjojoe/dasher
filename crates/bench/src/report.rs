//! Collecting results, printing them, and checking them against budgets.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use serde_json::{Value, json};

use crate::fixtures::Scenario;

const MIB: f64 = 1024.0 * 1024.0;

/// What one scenario cost, over all its runs: memory is the worst run,
/// time the median.
#[derive(Debug)]
pub struct PageResult {
    pub name: &'static str,
    pub description: &'static str,
    pub idle_rss: u64,
    pub peak_rss: u64,
    /// RSS with the page loaded, less the RSS before it was opened.
    pub page_rss: u64,
    pub after_drop_rss: u64,
    pub js_heap: u64,
    pub dom_nodes: u64,
    pub html_bytes: u64,
    pub settle_ms: f64,
}

#[derive(Debug)]
pub struct Results {
    pub allocator: &'static str,
    pub pages: Vec<PageResult>,
    /// The report of the sequential-jobs child.
    pub jobs: Option<Value>,
    pub binary_size: Option<u64>,
    pub version_ms: Option<f64>,
    pub cold_start_ms: Option<f64>,
}

fn field(run: &Value, name: &str) -> u64 {
    run[name].as_u64().unwrap_or(0)
}

fn max_of(runs: &[Value], f: impl Fn(&Value) -> u64) -> u64 {
    runs.iter().map(f).max().unwrap_or(0)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB
}

impl Results {
    pub fn new(allocator: &'static str) -> Self {
        Self {
            allocator,
            pages: Vec::new(),
            jobs: None,
            binary_size: None,
            version_ms: None,
            cold_start_ms: None,
        }
    }

    pub fn add_page(&mut self, scenario: &Scenario, runs: &[Value]) {
        self.pages.push(PageResult {
            name: scenario.name,
            description: scenario.description,
            idle_rss: max_of(runs, |r| field(r, "idle_rss")),
            peak_rss: max_of(runs, |r| field(r, "peak_rss")),
            page_rss: max_of(runs, |r| {
                field(r, "loaded_rss").saturating_sub(field(r, "idle_rss"))
            }),
            after_drop_rss: max_of(runs, |r| field(r, "after_drop_rss")),
            js_heap: max_of(runs, |r| field(r, "js_heap")),
            dom_nodes: max_of(runs, |r| field(r, "dom_nodes")),
            html_bytes: max_of(runs, |r| field(r, "html_bytes")),
            settle_ms: median(
                runs.iter()
                    .map(|r| r["settle_ms"].as_f64().unwrap_or(0.0))
                    .collect(),
            ),
        });
    }

    /// RSS of a ready, idle process: browser built and JS glue compiled,
    /// no page opened yet.
    pub fn idle_rss(&self) -> u64 {
        self.pages.iter().map(|p| p.idle_rss).max().unwrap_or(0)
    }

    fn job_rss(&self) -> Vec<u64> {
        self.jobs
            .as_ref()
            .and_then(|j| j["rss"].as_array())
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    }

    /// RSS after the 10th job (or the last, if there were fewer).
    pub fn rss_after_warmup(&self) -> Option<u64> {
        let rss = self.job_rss();
        rss.get(9).or(rss.last()).copied()
    }

    /// How much RSS grew from the 10th job to the last: what a long-lived
    /// process keeps that it should have given back.
    pub fn job_growth(&self) -> Option<i64> {
        let rss = self.job_rss();
        let last = *rss.last()?;
        let warm = self.rss_after_warmup()?;
        Some(last as i64 - warm as i64)
    }

    pub fn markdown(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "## dasher benchmarks\n");
        let _ = writeln!(
            s,
            "{}-{}, allocator: {}\n",
            std::env::consts::ARCH,
            std::env::consts::OS,
            self.allocator
        );
        let _ = writeln!(
            s,
            "| page | time to settle | peak RSS | page RSS | JS heap | DOM nodes | HTML out |"
        );
        let _ = writeln!(s, "| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
        for p in &self.pages {
            let _ = writeln!(
                s,
                "| **{}**: {} | {:.0} ms | {:.1} MiB | {:.1} MiB | {:.1} MiB | {} | {:.0} KiB |",
                p.name,
                p.description,
                p.settle_ms,
                mib(p.peak_rss),
                mib(p.page_rss),
                mib(p.js_heap),
                p.dom_nodes,
                p.html_bytes as f64 / 1024.0,
            );
        }
        let _ = writeln!(s);
        let _ = writeln!(s, "| measure | value |");
        let _ = writeln!(s, "| --- | ---: |");
        let _ = writeln!(
            s,
            "| idle RSS (ready, no page) | {:.1} MiB |",
            mib(self.idle_rss())
        );
        if let (Some(jobs), Some(warm), Some(growth)) =
            (&self.jobs, self.rss_after_warmup(), self.job_growth())
        {
            let n = jobs["jobs"].as_u64().unwrap_or(0);
            let last = self.job_rss().last().copied().unwrap_or(0);
            let _ = writeln!(
                s,
                "| {n} sequential jobs: RSS after job 10 → after job {n} | {:.1} → {:.1} MiB ({:+.2} MiB) |",
                mib(warm),
                mib(last),
                growth as f64 / MIB,
            );
            let _ = writeln!(
                s,
                "| {n} sequential jobs: peak RSS, total time | {:.1} MiB, {:.1} s |",
                mib(jobs["peak_rss"].as_u64().unwrap_or(0)),
                jobs["elapsed_ms"].as_f64().unwrap_or(0.0) / 1000.0,
            );
        }
        if let Some(ms) = self.version_ms {
            let _ = writeln!(s, "| process start (`dasher --version`) | {ms:.1} ms |");
        }
        if let Some(ms) = self.cold_start_ms {
            let _ = writeln!(
                s,
                "| cold start to static page loaded (`dasher URL`) | {ms:.1} ms |"
            );
        }
        if let Some(size) = self.binary_size {
            let _ = writeln!(s, "| `dasher` binary size | {:.2} MiB |", mib(size));
        }
        s
    }

    pub fn json(&self) -> Value {
        let pages: Vec<Value> = self
            .pages
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "idle_rss": p.idle_rss,
                    "peak_rss": p.peak_rss,
                    "page_rss": p.page_rss,
                    "after_drop_rss": p.after_drop_rss,
                    "js_heap": p.js_heap,
                    "dom_nodes": p.dom_nodes,
                    "html_bytes": p.html_bytes,
                    "settle_ms": p.settle_ms,
                })
            })
            .collect();
        json!({
            "target": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            "allocator": self.allocator,
            "idle_rss": self.idle_rss(),
            "pages": pages,
            "jobs": self.jobs,
            "job_growth": self.job_growth(),
            "binary_size": self.binary_size,
            "version_ms": self.version_ms,
            "cold_start_ms": self.cold_start_ms,
        })
    }
}

/// Limits the results must stay within, in MiB, from a JSON file:
///
/// ```json
/// {
///   "idle_rss_mib": 15,
///   "peak_rss_mib": { "static": 20, "spa": 80 },
///   "job_growth_mib": 2,
///   "binary_size_mib": 8
/// }
/// ```
///
/// Every key is optional. Times are not budgeted: they depend too much on
/// the machine.
#[derive(Debug)]
pub struct Budgets(Value);

impl Budgets {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
        Ok(Self(value))
    }

    /// The budgets the results are over, described.
    pub fn check(&self, results: &Results) -> Vec<String> {
        let mut over = Vec::new();
        let mut check = |what: &str, actual: f64, budget: Option<f64>| {
            if let Some(budget) = budget {
                if actual > budget {
                    over.push(format!("{what}: {actual:.2} MiB, budget {budget} MiB"));
                }
            }
        };
        let b = &self.0;
        check(
            "idle RSS",
            mib(results.idle_rss()),
            b["idle_rss_mib"].as_f64(),
        );
        for page in &results.pages {
            check(
                &format!("peak RSS of {}", page.name),
                mib(page.peak_rss),
                b["peak_rss_mib"][page.name].as_f64(),
            );
        }
        if let Some(growth) = results.job_growth() {
            check(
                "RSS growth over sequential jobs",
                growth as f64 / MIB,
                b["job_growth_mib"].as_f64(),
            );
        }
        if let Some(size) = results.binary_size {
            check("binary size", mib(size), b["binary_size_mib"].as_f64());
        }
        over
    }
}

pub fn verdict(over: &[String]) -> String {
    if over.is_empty() {
        "All results are within budget.".to_owned()
    } else {
        let mut s = String::from("**Over budget:**\n\n");
        for line in over {
            let _ = writeln!(s, "- {line}");
        }
        s
    }
}

/// Appends `text` to the file at `path`, creating it if need be.
pub fn append(path: &Path, text: &str) {
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = result {
        eprintln!("dasher-bench: cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::SCENARIOS;

    fn results() -> Results {
        let mut results = Results::new("system");
        let run = |peak: u64, settle: f64| {
            json!({
                "idle_rss": 10 << 20, "loaded_rss": 30 << 20, "peak_rss": peak,
                "after_drop_rss": 12 << 20, "js_heap": 4 << 20, "dom_nodes": 1000,
                "html_bytes": 2048, "settle_ms": settle,
            })
        };
        results.add_page(
            &SCENARIOS[2],
            &[
                run(40 << 20, 30.0),
                run(50 << 20, 10.0),
                run(45 << 20, 20.0),
            ],
        );
        let rss: Vec<u64> = (0..100).map(|i| (20 << 20) + i * 1024).collect();
        results.jobs =
            Some(json!({ "jobs": 100, "rss": rss, "peak_rss": 60 << 20, "elapsed_ms": 1000.0 }));
        results.binary_size = Some(5 << 20);
        results
    }

    #[test]
    fn runs_combine_as_worst_memory_and_median_time() {
        let page = &results().pages[0];
        assert_eq!(page.peak_rss, 50 << 20);
        assert_eq!(page.page_rss, 20 << 20);
        assert_eq!(page.settle_ms, 20.0);
    }

    #[test]
    fn job_growth_is_measured_from_the_tenth_job() {
        assert_eq!(results().job_growth(), Some(90 * 1024));
    }

    #[test]
    fn budgets_report_what_is_over() {
        let results = results();
        let budgets = Budgets(json!({
            "idle_rss_mib": 15,
            "peak_rss_mib": { "spa": 45, "static": 1 },
            "job_growth_mib": 1,
            "binary_size_mib": 4,
        }));
        let over = budgets.check(&results);
        assert_eq!(over.len(), 2, "{over:?}");
        assert!(over[0].starts_with("peak RSS of spa"));
        assert!(over[1].starts_with("binary size"));
        assert!(Budgets(json!({})).check(&results).is_empty());
        assert!(results.markdown().contains("| **spa**"));
    }
}
