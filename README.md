# dasher

A minimal headless "browser" written in Rust, used only for scraping.

dasher loads a URL and runs the page's JavaScript well enough for SPAs,
client-rendered content, infinite scroll and lazy-loading to fill in the DOM.
It then hands back the resulting HTML, or values pulled out with CSS selectors
or JS expressions.

It is built to run in serverless containers (Google Cloud Run, AWS Lambda), so
its priorities are, in order:

1. **Memory footprint**
2. **Cold-start time**
3. **Binary size**

Raw JavaScript speed matters less.

> Status: early development. The workspace is scaffolded; page loading is not
> implemented yet.

## Non-goals

dasher is not a general-purpose browser. It deliberately leaves out:

- rendering, layout and painting
- the CSS cascade and computed styles
- images, fonts, media, canvas and WebGL
- workers and service workers
- WebSockets
- iframes (for now)
- DevTools / the Chrome DevTools Protocol
- any UI

Only documents, scripts and fetch/XHR traffic go over the network. Images,
stylesheets, fonts and media are never downloaded.

Where pages expect layout (`getBoundingClientRect`, `offsetHeight`, scroll
position, `IntersectionObserver`, `getComputedStyle`), dasher returns plausible
synthetic values. They are chosen so that content gated behind visibility or
scrolling still loads: by default the page as it stood at the load event
counts as visible and in the viewport, so its lazy parts load once, and what
the page appends after that waits below the fold until dasher is asked to
scroll. See [Visibility](#visibility).

Content that web components render into shadow roots is part of the result:
the HTML carries each shadow tree as declarative shadow DOM
(`<template shadowrootmode="open">` inside its host), and CSS selectors
given to dasher look into shadow trees.

[COMPATIBILITY.md](COMPATIBILITY.md) lists the web APIs pages may expect that
dasher does not support yet.

## Bounded page loads

dasher waits for a page to go quiet (network idle and no DOM changes)
under a virtual clock that skips ahead to the page's next timer instead
of sleeping. Some pages never go quiet: carousels and tickers change the
DOM on a timer forever, and some feeds fetch more as soon as the last
batch arrives. Three budgets bound every load, and when one runs out the
page is returned as it stands, as a success that says which budget ended
it (`budget_exhausted` in the JSON output and the service's response,
`Navigation::budget_exhausted` in the Rust API):

| Budget | Default | CLI | Job field |
| --- | --- | --- | --- |
| Page time spent waiting for quiet: skipped, or idle on timers | 5 s | `--page-time-budget` | `page_time_budget_ms` |
| CPU time for loading, settling and scrolling | 10 s | `--cpu-budget` | `cpu_budget_ms` |
| Requests the page makes | 1000 | `--request-budget` | `request_budget` |

Time spent waiting for the network or running scripts does not count as
page time. `requestIdleCallback` work never holds a page open.

## Design

dasher's own code covers the DOM, the event loop, the JS bindings, the page
lifecycle and the network policy. Spec-heavy commodity parts come from mature
crates:

- **JavaScript:** [QuickJS](https://bellard.org/quickjs/) via
  [rquickjs](https://crates.io/crates/rquickjs), chosen over V8 for memory and
  size. The engine is built from [`vendor/rquickjs-sys`](vendor/rquickjs-sys),
  which carries a few changes of dasher's own; its
  [`PATCHES.md`](vendor/rquickjs-sys/PATCHES.md) lists them. Its heap is a
  slab allocator of dasher's (`crates/js/src/slab.rs`), not `malloc`.
- **HTML parsing:** [html5ever](https://crates.io/crates/html5ever) tokenizes
  and builds the tree, into dasher's own arena DOM.
- **HTTP:** [hyper](https://crates.io/crates/hyper) with
  [rustls](https://crates.io/crates/rustls) (no OpenSSL), on
  [tokio](https://crates.io/crates/tokio).

Data structures favour compactness: arena/index-based nodes, interned names,
no per-node reference counting. Each `Page` owns its JS runtime and DOM, and
dropping the page frees all of it. The target artifact is a single static
binary (musl) in a distroless or scratch-sized container.

## Visibility

What counts as in view decides how much lazily loaded content a page
fetches, and so what a run costs. It is a named policy, set per run:
`--visibility` in the CLI, `"visibility"` in a service job, and
`BrowserBuilder::visibility(Visibility::…)` in the Rust API.

| policy | what is in view | what it costs |
| --- | --- | --- |
| `loaded` (default) | The page as it stood at the load event. What it appends later is below the fold, pushing what follows it down, until `--scroll` reveals it. | Lazy sections, images and components present at load arrive. A loader can add about two viewports of content before the next scroll, so a feed loads one batch, and a page that checks for more after every batch (as engadget.com does) stops. |
| `all` | Everything, always. | The most content, but a page that checks for more after every batch keeps loading until the timeout: engadget.com made 4,800 to 8,000 requests and spent 38 to 60 CPU seconds, against under 2 s with the other policies. |
| `viewport` | Only what is scrolled to, in a 1920×1080 window. | The least work. Content below the fold arrives only with `--scroll`. |

`--scroll` is the way to ask for more of an infinite feed under any
policy: each round reveals what came in and scrolls down again, up to
`--max-scrolls` rounds.

On sixteen recorded real pages (see
[crates/bench/README.md](crates/bench/README.md#visibility)), `loaded` kept
the same text as `all` on fifteen and cost 36 CPU seconds in all, against 77
for `all` and 34 for `viewport`. `viewport` kept the same text as well,
except on engadget.com (1,615 words, against 4,128 under `loaded`). The
difference is all on pages with self-triggering loaders: elsewhere the three
policies cost the same.

## Workspace layout

| Crate | Path | Purpose |
| --- | --- | --- |
| `dasher-net` | `crates/net` | HTTP client and network policy |
| `dasher-dom` | `crates/dom` | Arena DOM and HTML parsing |
| `dasher-css` | `crates/css` | CSS selector parsing and matching |
| `dasher-js` | `crates/js` | QuickJS runtime, event loop and DOM bindings |
| `dasher-page` | `crates/page` | Page lifecycle and orchestration |
| `dasher` | `crates/dasher` | Public library API |
| `dasher-serve` | `crates/serve` | HTTP service and AWS Lambda runtime |
| `dasher-cli` | `crates/cli` | The `dasher` binary: CLI, `serve` and `lambda` |
| `dasher-test-server` | `crates/test-server` | In-process HTTP fixture server for tests |
| `dasher-bench` | `crates/bench` | Memory, time and size benchmarks on a fixture suite |

## Deploying

The `dasher` binary is also the service. `dasher serve` takes JSON jobs
at `POST /scrape` (a URL, how to wait, what to extract) and answers with
JSON, for Cloud Run or any container platform; `dasher lambda` takes the
same jobs as an AWS Lambda custom runtime. The [`Dockerfile`](Dockerfile)
builds a distroless image with the static binary.

```sh
docker build -t dasher . && docker run --rm -p 8080:8080 dasher
curl -s localhost:8080/scrape -d '{"url": "https://example.com/", "extract": {"title": "h1"}}'
```

The service refuses to let pages connect to loopback, private and
link-local addresses (the cloud metadata server among them), checked
after DNS; set `DASHER_ALLOW_PRIVATE_ADDRESSES=true` to scrape a server
on your own machine. See [docs/deploy.md](docs/deploy.md) for Cloud Run
and Lambda, the job format, sizing and security.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release   # size-tuned: LTO, 1 codegen unit, panic=abort, stripped
```

All three checks must pass on `main`; CI runs them on every push and pull
request, along with a static musl release build.

Behaviour that needs a page is tested against local fixture HTML/JS served by
`dasher-test-server`, never against live websites. A crate's fixtures live in
its `tests/fixtures` directory:

```rust
use std::time::Duration;
use dasher_test_server::{fixtures_dir, FixtureResponse, FixtureServer};

let server = FixtureServer::builder()
    .root(fixtures_dir!())
    .path_latency("/slow.js", Duration::from_millis(200))
    .route("/api/items", |_req| FixtureResponse::json(r#"[1, 2, 3]"#))
    .start()?;

// ... load server.url("/index.html") ...

assert_eq!(server.request_count_for("/api/items"), 1);
assert_eq!(server.request_count_for("/hero.png"), 0); // never downloaded
```

Add it to a crate with:

```toml
[dev-dependencies]
dasher-test-server.workspace = true
```
