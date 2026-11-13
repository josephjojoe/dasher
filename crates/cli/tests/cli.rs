//! The `dasher` binary, run against local fixtures.

use std::process::{Command, Output};
use std::time::Duration;

use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};
use serde_json::Value;

fn server() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/data", |req| {
            let header = |name| req.header(name).unwrap_or_default().to_owned();
            let body = serde_json::json!({
                "items": ["alpha", "beta"],
                "headers": {
                    "user-agent": header("user-agent"),
                    "x-token": header("x-token"),
                    "cookie": header("cookie"),
                },
            });
            FixtureResponse::json(body.to_string())
        })
        .route("/api/feed", |req| {
            let page = req.query.clone().unwrap_or_default();
            FixtureResponse::json(format!(r#"["{page} a", "{page} b"]"#))
        })
        .path_latency("/api/data", Duration::from_millis(50))
        .start()
        .unwrap()
}

fn dasher(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dasher"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).unwrap()
}

#[test]
fn prints_the_rendered_html() {
    let server = server();
    let html = stdout(&dasher(&[&server.url("/app.html")]));
    assert!(
        html.starts_with("<!DOCTYPE html><html><head><title>App</title>"),
        "{html}"
    );
    assert!(
        html.contains(r#"<div id="app"><p class="item">alpha</p><p class="item">beta</p></div>"#),
        "{html}"
    );
    assert!(html.ends_with("</html>\n"));
}

#[test]
fn wait_strategies_choose_when_to_stop() {
    let server = server();
    let url = server.url("/app.html");
    let at_load = stdout(&dasher(&[&url, "--wait", "load"]));
    assert!(!at_load.contains(r#"id="late""#));
    // The default settle does not wait two seconds for a timer...
    let settled = stdout(&dasher(&[&url]));
    assert!(!settled.contains(r#"id="late""#));
    // ...but a selector does, and the virtual clock makes it quick.
    let waited = stdout(&dasher(&[&url, "--wait-for", "#late"]));
    assert!(waited.contains(r#"<p id="late">late</p>"#), "{waited}");
    let waited = stdout(&dasher(&[
        &url,
        "--wait-until",
        "!!document.getElementById('late')",
    ]));
    assert!(waited.contains(r#"<p id="late">late</p>"#));
}

#[test]
fn eval_prints_strings_raw_and_other_values_as_json() {
    let server = server();
    let url = server.url("/app.html");
    assert_eq!(
        stdout(&dasher(&[&url, "--eval", "document.title"])),
        "App\n"
    );
    assert_eq!(
        stdout(&dasher(&[
            &url,
            "-e",
            "[...document.querySelectorAll('.item')].map(p => p.textContent)"
        ])),
        "[\"alpha\",\"beta\"]\n"
    );
    assert_eq!(
        stdout(&dasher(&[
            &url,
            "-e",
            "async () => (await fetch('/api/data')).status"
        ])),
        "200\n"
    );
}

#[test]
fn json_output_describes_the_run() {
    let server = server();
    let url = server.url("/app.html");
    let out = json(&dasher(&[&url, "--json"]));
    assert_eq!(out["url"], url.as_str());
    assert_eq!(out["status"], 200);
    assert_eq!(out["title"], "App");
    assert_eq!(out["timed_out"], false);
    assert_eq!(out["settle"], "settled");
    assert_eq!(out["scroll"], Value::Null);
    assert!(out["html"].as_str().unwrap().contains("beta"));
    assert_eq!(
        out["console"],
        serde_json::json!([{"level": "log", "text": "booting"}])
    );
    assert_eq!(out["errors"], serde_json::json!([]));

    let out = json(&dasher(&[
        &url,
        "--json",
        "--eval",
        "document.querySelectorAll('.item').length",
    ]));
    assert_eq!(out["result"], 2);
    assert!(out.get("html").is_none());
}

#[test]
fn browser_options_reach_the_page_and_the_network() {
    let server = server();
    let url = server.url("/app.html");
    let out = json(&dasher(&[
        &url,
        "--json",
        "-A",
        "TestBot/2.0",
        "-H",
        "X-Token: secret",
        "--cookie",
        "session=abc; Path=/",
        "--viewport",
        "390x844",
        "--block",
        "*/ads/*",
        "-e",
        "({headers, ua: navigator.userAgent, width: innerWidth, tracked: !!window.tracked})",
    ]));
    assert_eq!(
        out["result"],
        serde_json::json!({
            "headers": {"user-agent": "TestBot/2.0", "x-token": "secret", "cookie": "session=abc"},
            "ua": "TestBot/2.0",
            "width": 390,
            "tracked": false,
        })
    );
    assert_eq!(server.request_count_for("/ads/tracker.js"), 0);
}

#[test]
fn scroll_loads_more_of_a_feed() {
    let server = server();
    let url = server.url("/feed.html");
    let count = "document.querySelectorAll('article').length";
    assert_eq!(stdout(&dasher(&[&url, "-e", count])), "1\n");
    assert_eq!(stdout(&dasher(&[&url, "--scroll", "-e", count])), "7\n");
    // One round steps a viewport down and then nudges at the bottom: two
    // scroll events, and this feed loads a page on each.
    assert_eq!(
        stdout(&dasher(&[&url, "--max-scrolls", "1", "-e", count])),
        "5\n"
    );
    // Under `all` the one-post document is a single viewport tall, so
    // there is only the nudge.
    assert_eq!(
        stdout(&dasher(&[
            &url,
            "--visibility",
            "all",
            "--max-scrolls",
            "1",
            "-e",
            count
        ])),
        "3\n"
    );
    // Its three pages come in over two rounds.
    let out = json(&dasher(&[&url, "--scroll", "--json", "-e", "1"]));
    assert_eq!(out["scroll"]["productive_scrolls"], 2);
    assert_eq!(out["scroll"]["stopped"], "no_new_content");
}

#[test]
fn verbose_reports_console_output_on_stderr() {
    let server = server();
    let output = dasher(&[&server.url("/app.html"), "-v", "-e", "1"]);
    assert_eq!(stdout(&output), "1\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("console.log: booting"), "{stderr}");
}

#[test]
fn failures_set_the_exit_status() {
    let server = server();
    let url = server.url("/app.html");

    let output = dasher(&[&url, "-e", "nope()"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ReferenceError: nope is not defined"),
        "{stderr}"
    );

    let output = dasher(&[&url, "--json", "-e", "nope()"]);
    assert_eq!(output.status.code(), Some(1));
    let out: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(out["error"].as_str().unwrap().contains("ReferenceError"));
    assert_eq!(out["status"], 200);

    // Nothing listens on port 9 of localhost.
    let output = dasher(&["http://127.0.0.1:9/", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let out: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(out["url"], "http://127.0.0.1:9/");
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("loading the document failed")
    );

    assert_eq!(dasher(&["ftp://example.com/"]).status.code(), Some(1));
    assert_eq!(dasher(&[]).status.code(), Some(2));
    assert_eq!(dasher(&[&url, "--wait", "never"]).status.code(), Some(2));
    assert_eq!(
        dasher(&[&url, "-H", "bad header: x"]).status.code(),
        Some(2)
    );
}

#[test]
fn pages_over_their_limits_fail_cleanly() {
    let server = server();
    let url = server.url("/app.html");

    let output = dasher(&[&url, "--max-dom-nodes", "10"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the page went over its limit of 10 DOM nodes"),
        "{stderr}"
    );

    let output = dasher(&[
        &url,
        "--memory-limit",
        "16",
        "--json",
        "-e",
        "const a = []; for (;;) a.push('x'.repeat(4096) + a.length)",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let out: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        out["error"], "the page ran out of JS heap (limit 16 MiB)",
        "{out}"
    );

    // 0 lifts a limit.
    stdout(&dasher(&[
        &url,
        "--max-dom-nodes",
        "0",
        "--memory-limit",
        "0",
    ]));
}

#[test]
fn timeouts_return_the_page_as_far_as_it_got() {
    let server = server();
    let output = dasher(&[
        &server.url("/app.html"),
        "--realtime",
        "--wait-for",
        "#never",
        "--timeout",
        "0.5",
        "-e",
        "document.title",
    ]);
    assert_eq!(stdout(&output), "App\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "{stderr}");
}

#[test]
fn help_and_version() {
    let help = stdout(&dasher(&["--help"]));
    assert!(help.starts_with("usage: dasher [options] <url>"));
    let version = stdout(&dasher(&["--version"]));
    assert_eq!(version, format!("dasher {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn budgets_end_pages_that_never_finish_and_say_so() {
    let server = server();
    let out = json(&dasher(&[&server.url("/ticker.html"), "--json"]));
    assert_eq!(out["timed_out"], false);
    assert_eq!(out["settle"], "budget_exhausted");
    assert_eq!(out["budget_exhausted"], "page_time");
    assert!(out["html"].as_str().unwrap().contains("Headline"));

    let out = json(&dasher(&[
        &server.url("/more.html"),
        "--json",
        "--request-budget",
        "20",
    ]));
    assert_eq!(out["budget_exhausted"], "requests");
    assert!(out["requests"].as_u64().unwrap() <= 22, "{out}");

    // With the budgets off, the page runs until the timeout.
    let out = json(&dasher(&[
        &server.url("/ticker.html"),
        "--json",
        "--page-time-budget",
        "0",
        "-t",
        "0.5",
    ]));
    assert_eq!(out["timed_out"], true);
    assert_eq!(out["budget_exhausted"], Value::Null);
}
