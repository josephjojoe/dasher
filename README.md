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
scrolling still loads: by default everything counts as visible and in the
viewport, and the document is treated as tall enough to scroll.

[COMPATIBILITY.md](COMPATIBILITY.md) lists the web APIs pages may expect that
dasher does not support yet.

## Design

dasher's own code covers the DOM, the event loop, the JS bindings, the page
lifecycle and the network policy. Spec-heavy commodity parts come from mature
crates:

- **JavaScript:** [QuickJS](https://bellard.org/quickjs/) via
  [rquickjs](https://crates.io/crates/rquickjs), chosen over V8 for memory and
  size.
- **HTML parsing:** [html5ever](https://crates.io/crates/html5ever) tokenizes
  and builds the tree, into dasher's own arena DOM.
- **HTTP:** [hyper](https://crates.io/crates/hyper) with
  [rustls](https://crates.io/crates/rustls) (no OpenSSL), on
  [tokio](https://crates.io/crates/tokio).

Data structures favour compactness: arena/index-based nodes, interned names,
no per-node reference counting. Each `Page` owns its JS runtime and DOM, and
dropping the page frees all of it. The target artifact is a single static
binary (musl) in a distroless or scratch-sized container.

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

See [docs/deploy.md](docs/deploy.md) for Cloud Run and Lambda, the job
format and sizing.

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
