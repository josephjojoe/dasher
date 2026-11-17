# Changelog

## 0.1.0 (2026-11-17)

The first release.

- **One static binary, three ways to run it.** `dasher <url>` prints a
  page's HTML, or the value of a JavaScript expression, once its scripts
  have run and it has settled. `dasher serve` is an HTTP service taking
  JSON jobs at `POST /scrape`. `dasher lambda` is an AWS Lambda custom
  runtime taking the same jobs.
- **A Rust library**, `dasher`: `Browser`, `Page`, selector helpers,
  `evaluate`, waiting and auto-scroll.
- **Page loading** as a browser does it, minus rendering: classic and
  module scripts, `defer` and `async`, `document.write`, import maps,
  dynamic `import()`, script navigations and meta refreshes, cookies,
  `fetch` and XHR, web components and shadow DOM.
- **Bounded loads.** A time limit, and budgets for page time, CPU,
  requests and the real time a page is waited for (5 s by default); a
  page that runs out of one is returned as it stands. Caps on the JS
  heap and the DOM stop a runaway page without taking the process down.
- **Visibility policies** (`loaded`, `all`, `viewport`) decide how much
  lazy content loads, and `--scroll` works through infinite feeds.
- **A script cache** keeps compiled scripts across pages, in memory and
  optionally on disk.
- **For services**: a concurrency cap and queue, graceful shutdown,
  bearer-token auth, and refusal of loopback, link-local and private
  addresses, checked after DNS and on every redirect.
- **A patched QuickJS-NG** (`vendor/rquickjs-sys`): lazy function
  compilation, faster regular expressions, JSON, sorting and object
  creation.

Measured against Lightpanda and Chromium on 492 live sites:
[crates/bench/lambda](crates/bench/lambda/README.md). Known gaps:
[COMPATIBILITY.md](COMPATIBILITY.md).
