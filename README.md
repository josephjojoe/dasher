# dasher

A minimal headless browser for scraping, written in Rust.

dasher loads a URL and runs the page's JavaScript well enough for
single-page apps, client-rendered content, lazy loading and infinite
scroll to fill in the DOM. Then it hands back the HTML, or values pulled
out of the page with CSS selectors or JavaScript. It renders nothing.

It is one static binary of about 6 MiB that is a command-line tool, an
HTTP service and an AWS Lambda runtime, built for serverless containers,
where memory and CPU seconds are the bill.

```console
$ dasher -e 'document.title' https://example.com/
Example Domain
$ dasher -e '[...document.querySelectorAll(".titleline > a")].slice(0, 2).map(a => a.textContent)' https://news.ycombinator.com/
["Old Bitly Links Are Being Hijacked","Preserving 30 Years of Salon Journalism"]
```

## How it compares

On 492 popular sites loaded live from AWS Lambda (arm64, 3008 MB),
three runs each, with [Lightpanda](https://github.com/lightpanda-io/browser)
and Chromium's headless shell run back to back in the same sandbox
(2026-11-15). The figures are for the 328 sites all three loaded:

| | dasher 0.1.0 | Lightpanda nightly | Chromium 156 |
| --- | ---: | ---: | ---: |
| Peak memory, median page | 39 MiB | 94 MiB | 387 MiB |
| Peak memory, 90th percentile | 120 MiB | 209 MiB | 671 MiB |
| CPU, median page | 0.94 s | 1.05 s | 3.74 s |
| CPU, all pages | 612 s | 590 s | 2168 s |
| Wall time, median page | 2.0 s | 2.7 s | 5.4 s |
| Sites loaded, of 492 | 372 | 390 | 355 |
| Start-up on an empty page: CPU, memory | 11 ms, 7 MiB | 39 ms, 23 MiB | 270 ms, 193 MiB |

- **Memory is the reason to use dasher**: four tenths of Lightpanda's
  peak on the median site and a tenth of Chromium's.
- **CPU is level with Lightpanda** (4% more over all pages) and about a
  quarter of Chromium's. Where the two load the same content with the
  same requests, dasher uses 1.17× Lightpanda's CPU: QuickJS interprets
  and V8 compiles.
- **It loads fewer sites than Lightpanda.** A fifth of the list answers
  requests from AWS with a bot wall, and dasher does nothing to get
  past one: 93 sites blocked it, 76 blocked Lightpanda and 111 blocked
  Chromium.
- **Text matches Chromium's** on most sites: at least 0.9 similar on
  252 of the 333 both loaded (Lightpanda: 260 of 344), and under half
  the text on 13.
- **It starts sooner than Lightpanda**: 11 ms of CPU before a page's
  own work, against 39 ms.

Method, per-site results and what the comparison does not show:
[crates/bench/lambda](crates/bench/lambda/README.md).

## Install

Static Linux binaries (x86_64 and arm64), a macOS binary (Apple
silicon) and Lambda zips are on the
[releases page](https://github.com/josephjojoe/dasher/releases). Or
build from source with Rust 1.88 or later and a C compiler:

```sh
cargo install --locked --git https://github.com/josephjojoe/dasher dasher-cli
```

Or as a container, which runs the HTTP service:

```sh
docker build -t dasher https://github.com/josephjojoe/dasher.git
docker run --rm -p 8080:8080 dasher
```

## Use

### Command line

```sh
dasher https://example.com/                      # the HTML, once the page is quiet
dasher -e 'document.title' https://example.com/  # the value of an expression
dasher --json https://example.com/               # HTML, status, console, errors, as JSON
dasher --wait-for '.results li' --scroll https://example.com/feed
```

`--wait` chooses when a page counts as ready (`load`, `network-idle`,
`dom-quiet`, `settled`), `--wait-for` and `--wait-until` wait for a
selector or an expression, and `--scroll` scrolls through infinite
feeds. Headers, cookies, a proxy, blocked URL patterns, the viewport and
the user agent are options too: `dasher --help`.

### HTTP service

```sh
dasher serve --port 8080 &
curl -s localhost:8080/scrape -d '{
  "url": "https://news.ycombinator.com/",
  "extract": {
    "first": ".titleline",
    "links": {"selector": ".titleline > a", "attribute": "href", "all": true},
    "count": {"selector": ".athing", "count": true}
  }
}'
```

A job is a URL, how to wait and what to extract; the answer is JSON.
The service caps concurrency, queues briefly, drains on `SIGTERM` and
refuses to let pages reach private addresses. `dasher lambda` takes the
same jobs as an AWS Lambda custom runtime. The job format, Cloud Run and
Lambda set-up, sizing and security are in
[docs/deploy.md](docs/deploy.md).

### Rust

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() -> dasher::Result<()> {
    let browser = dasher::Browser::new()?;
    let mut page = browser.open("https://news.ycombinator.com/").await?;

    let titles: Vec<String> = page.texts(".titleline > a")?;
    let links: Vec<String> = page.attributes(".titleline > a", "href")?;
    let count: usize = page.evaluate_as("document.querySelectorAll('.athing').length").await?;
    println!("{count} stories, first: {} ({})", titles[0], links[0]);
    Ok(())
}
```

dasher is not on crates.io: it builds against a patched QuickJS
([`vendor/rquickjs-sys`](vendor/rquickjs-sys/PATCHES.md)), and a
published crate cannot carry a `[patch]`. The crate named `dasher`
there is an unrelated project, so `cargo add dasher` and
`cargo install dasher` get something else. Depend on the repository and
repeat the patch:

```toml
[dependencies]
dasher = { git = "https://github.com/josephjojoe/dasher", tag = "v0.1.0" }
tokio = { version = "1", features = ["macros", "rt"] }

[patch.crates-io]
rquickjs-sys = { git = "https://github.com/josephjojoe/dasher", tag = "v0.1.0" }
```

The API is documented in the crate (`cargo doc -p dasher --open`), with
two examples in [`crates/dasher/examples`](crates/dasher/examples).

## What it is not

dasher is not a general-purpose browser. It leaves out:

- rendering, layout and painting
- stylesheets, the cascade and computed styles
- images, fonts and media
- WebAssembly, workers and service workers
- WebSockets and iframes
- the Chrome DevTools Protocol, and any UI
- anything meant to defeat bot detection

Only documents, scripts and what scripts request go over the network.

Where pages expect layout (`getBoundingClientRect`, `offsetHeight`,
scroll positions, `IntersectionObserver`, `getComputedStyle`), dasher
answers with plausible synthetic values, chosen so that content gated
behind visibility or scrolling still loads.

Content that web components render into shadow roots is part of the
result: the HTML carries each shadow tree as declarative shadow DOM, and
selectors given to dasher look into shadow trees.

[COMPATIBILITY.md](COMPATIBILITY.md) lists what a page sees: the APIs
that are there, the stand-ins, and the gaps.

## Bounded page loads

dasher waits for a page to go quiet (by default, no requests for
500 ms) under a clock that skips ahead to the page's next timer instead
of sleeping. Some pages never go quiet: a carousel changes the DOM on a
timer forever, a feed fetches more as soon as the last batch arrives.
Three budgets bound every load, and when one runs out the page is
returned as it stands, as a success that names the budget:

| Budget | Default | CLI | Job field |
| --- | --- | --- | --- |
| Page time spent waiting for quiet | 5 s | `--page-time-budget` | `page_time_budget_ms` |
| CPU time for loading, settling and scrolling | 10 s | `--cpu-budget` | `cpu_budget_ms` |
| Requests the page makes | 1000 | `--request-budget` | `request_budget` |

A page's JS heap is capped as well (256 MiB), and its DOM (500,000
nodes); a page that goes over is stopped and the load fails, so one
runaway page cannot take a process down.

## Visibility

What counts as in view decides how much lazily loaded content a page
fetches, and so what a load costs. It is a policy, set per run with
`--visibility`, a job's `"visibility"` or `BrowserBuilder::visibility`:

| Policy | What is in view | What it costs |
| --- | --- | --- |
| `loaded` (default) | The page as it stood at the load event. What it adds later is below the fold until a scroll reveals it. | Lazy sections present at load arrive, and a feed loads one batch. |
| `all` | Everything, always. | The most content, but a page that checks for more after every batch keeps loading until the time limit. |
| `viewport` | Only what is scrolled to, in a 1920×1080 window. | The least work; content below the fold arrives only with `--scroll`. |

`--scroll` asks for more of an infinite feed under any policy: each
round reveals what came in and scrolls again, up to `--max-scrolls`.

## Design

dasher's own code is the DOM, the event loop, the JS bindings, the page
lifecycle and the network policy. The spec-heavy parts come from mature
crates:

- **JavaScript:** [QuickJS-NG](https://github.com/quickjs-ng/quickjs)
  through [rquickjs](https://crates.io/crates/rquickjs), chosen over V8
  for memory and size. The engine is built from
  [`vendor/rquickjs-sys`](vendor/rquickjs-sys), which carries dasher's
  changes to it: lazy function compilation, faster regular expressions,
  JSON, sorting and object creation, and support for a script cache.
  [`PATCHES.md`](vendor/rquickjs-sys/PATCHES.md) lists them. Its heap is
  a slab allocator of dasher's own.
- **HTML:** [html5ever](https://crates.io/crates/html5ever) parses into
  dasher's arena DOM.
- **HTTP:** [hyper](https://crates.io/crates/hyper) with
  [rustls](https://crates.io/crates/rustls) on
  [tokio](https://crates.io/crates/tokio). No OpenSSL.

Each page owns its JS runtime and DOM, and dropping it frees both.
Compiled scripts are kept across pages in a bounded cache keyed by a
hash of each script's text, so a site visited again, or a library two
sites share, is not compiled twice.

| Crate | Path | Purpose |
| --- | --- | --- |
| `dasher-net` | `crates/net` | HTTP client and network policy |
| `dasher-dom` | `crates/dom` | Arena DOM and HTML parsing |
| `dasher-css` | `crates/css` | CSS selector parsing and matching |
| `dasher-js` | `crates/js` | QuickJS runtime, event loop and DOM bindings |
| `dasher-page` | `crates/page` | Page lifecycle: loading, settling, scrolling |
| `dasher` | `crates/dasher` | The public library API |
| `dasher-serve` | `crates/serve` | HTTP service and AWS Lambda runtime |
| `dasher-cli` | `crates/cli` | The `dasher` binary |
| `dasher-test-server` | `crates/test-server` | In-process HTTP fixture server for tests |
| `dasher-bench` | `crates/bench` | Footprint, micro and comparison benchmarks |

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release   # size-tuned: LTO, one codegen unit, panic=abort, stripped
```

CI runs these on every push and pull request, with `cargo deny`, a
static musl build held to the memory and size budgets in
[`crates/bench/budgets.json`](crates/bench/budgets.json), and a build of
the container image.

Behaviour that needs a page is tested against local fixtures served by
`dasher-test-server`, never against live sites. The compatibility suite
loads production builds of React, Next.js, Vue, Vite, jQuery, React
Router and Lit ([crates/page/tests/compat](crates/page/tests/compat/README.md)).
Benchmarks are described in [crates/bench](crates/bench/README.md).

## Security

dasher runs untrusted JavaScript in an interpreter with capped memory
and CPU, with no file or process access from the page. The service
refuses private addresses, so a page cannot reach the network it runs
in; see [docs/deploy.md](docs/deploy.md#security). To report a
vulnerability, see [SECURITY.md](SECURITY.md).

## Licence

MIT; see [LICENSE](LICENSE). The binary includes the work of others
under their own licences, listed in
[THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt).
