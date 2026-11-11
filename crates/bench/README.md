# dasher-bench

Measures dasher's footprint (memory, time and binary size) on a fixed
fixture suite, and checks the results against [`budgets.json`](budgets.json).
CI runs it on every push, on the static musl release build, and fails if a
budget is exceeded. The results show up in the job summary and are
uploaded as the `benchmarks` artifact.

```sh
cargo build --release -p dasher-cli -p dasher-bench
target/release/dasher-bench --budgets crates/bench/budgets.json
```

Options: `--runs N` sets how many processes run each page (default 3),
`--jobs N` sets the length of the sequential-jobs run (default 100),
`--json PATH` and `--markdown PATH` write the results to files, and
`--cli PATH` points at the `dasher` binary to time and weigh. By default
that is the binary next to `dasher-bench`.

## The fixtures

Every fixture is compiled into the binary and served from an in-process
server with 2 ms of latency per response. Nothing touches the network.
Each page must render the expected number of elements, so a page that
breaks fails the run instead of looking cheap.

| page | what it is |
| --- | --- |
| `static` | an 11 KB article with no scripts |
| `large` | a 1.6 MB catalogue: a 5000-row table (75,000 DOM nodes) with no scripts |
| `spa` | a React 18 shop (production UMD builds, vendored under MIT). It fetches 100 products from a JSON API, renders them through components with state, context and a reducer, and loads reviews for the first 10 |
| `feed` | an infinite feed: an `IntersectionObserver` on a sentinel loads 20 pages of 20 posts, driven by auto-scroll |

## What is measured

Each page loads in a fresh child process, so its peak RSS is its own. The
child builds a `Browser` and calls `dasher::precompile()`, as a service
would at start-up. Then it loads the page with the default `goto` (load
event, then 500 ms each of network idle and DOM quiet), plus auto-scroll
for `feed`. Finally it serializes the HTML.

- **time to settle**: wall time of that `goto`. Under the default virtual
  clock, quiet periods take no real time, so this is CPU time plus fixture
  latency.
- **peak RSS**: the child process's high-water mark (`VmHWM`), for the
  whole job.
- **page RSS**: RSS with the page loaded, less RSS just before it was
  opened.
- **JS heap** and **DOM nodes**: `Page::memory_usage()`.
- **idle RSS**: a process that is ready for work, with the browser built and
  the glue compiled, but no page opened.
- **sequential jobs**: one process, one browser, 100 jobs that cycle
  through the four pages. Each job runs on a new page that is dropped
  afterwards, and RSS is sampled after every job. The first 10 jobs are a
  warm-up while pools and caches fill. Growth is the highest RSS of the
  second half of the remaining jobs minus that of the first half. RSS
  swings by a few hundred KiB with the mix of pages, so comparing single
  samples would mostly measure where each one fell in the cycle.
- **process start** and **cold start**: the median wall time of `dasher
  --version`, and of `dasher <static page URL>`, each run as a fresh
  process.
- **binary size**: the `dasher` binary.

`crates/dasher/tests/memory.rs` checks the same "memory comes back"
property exactly: a counting allocator must see the live Rust heap stay
flat over 100 jobs once it has warmed up. The JS heap is freed with each
page's runtime, which dasher-js tests.

## Baseline

Static musl release build (`aarch64-unknown-linux-musl`), run in Docker on
an Apple M2 Pro, 3 runs per page, 2026-11-10:

| page | time to settle | peak RSS | page RSS | JS heap | DOM nodes | HTML out |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| static | 11 ms | 6.4 MiB | 2.8 MiB | 1.6 MiB | 289 | 11 KiB |
| large | 53 ms | 16.8 MiB | 9.8 MiB | 1.6 MiB | 75,194 | 1698 KiB |
| spa | 92 ms | 11.4 MiB | 7.4 MiB | 5.1 MiB | 2,291 | 76 KiB |
| feed | 132 ms | 8.9 MiB | 4.8 MiB | 2.5 MiB | 8,830 | 271 KiB |

| measure | value |
| --- | ---: |
| idle RSS (ready, no page) | 3.7 MiB |
| 100 sequential jobs: highest RSS between jobs, first half → second half | 6.0 → 6.1 MiB (+0.01 MiB) |
| 100 sequential jobs: peak RSS, total time | 17.7 MiB, 7.9 s |
| process start (`dasher --version`) | 0.4 ms |
| cold start to static page loaded | 37.5 ms |
| `dasher` binary size | 4.63 MiB |

On macOS (aarch64, release build, system allocator) the same suite gives
6.6 MiB idle, peak RSS of 9.8, 24.0, 14.8 and 12.2 MiB for the four pages,
and a 4.33 MiB binary. Those numbers also fit the budgets, so the check can
be run locally.

### Targets

The initial targets were under 15 MB RSS idle and under 80 MB peak for a
typical SPA page. Both are met by a wide margin: 3.7 MiB idle, and
11.4 MiB peak for the React shop. The targets stay as the product
promise. The CI budgets in `budgets.json` sit much closer to the
measurements, at about 1.3 to 2 times each one, so a regression shows up
long before the targets are at risk:

| budget | value | measured |
| --- | ---: | ---: |
| idle RSS | 8 MiB | 3.7 MiB (6.7 MiB with glibc) |
| peak RSS: static, large, spa, feed | 12, 28, 20, 16 MiB | 6.4, 16.8, 11.4, 8.9 MiB |
| RSS growth over sequential jobs | 0.75 MiB | 0.01 to 0.07 MiB |
| binary size | 6 MiB | 4.63 MiB |

Times are reported but not budgeted, because they depend too much on the
runner.

Real SPAs are heavier than the fixture: bigger bundles and more data. The
fixture needs a 5 MiB JS heap and 11 MiB of RSS in all, which leaves room
for several times its script and state within 80 MB. Pages that go far
beyond that are what the per-page limits below guard against.

## Allocator

dasher keeps the **system allocator**. In the static musl build that ships,
this is musl's own `malloc`. QuickJS allocates through the C `malloc`
too, so one allocator serves the whole process.

The alternatives were measured with the same suite. They were built with
`#[global_allocator]`, plus `malloc` overriding so that QuickJS's heap
moved with them: mimalloc 0.1 with `override`, and tikv-jemallocator 0.6
with `unprefixed_malloc_on_supported_platforms`. Each also ran tuned for
low RSS. For mimalloc that meant `MIMALLOC_PURGE_DELAY=0`,
`MIMALLOC_ARENA_EAGER_COMMIT=0` and `MIMALLOC_ALLOW_LARGE_OS_PAGES=0`. For
jemalloc it meant `MALLOC_CONF=narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0`.
Results, on aarch64 Linux:

| allocator | idle RSS | peak RSS static / large / spa / feed | RSS between jobs (highest) | cold start | binary |
| --- | ---: | ---: | ---: | ---: | ---: |
| **musl malloc (chosen)** | **3.8 MiB** | **6.4 / 16.8 / 11.2 / 9.0** | **5.9 MiB** | 62 ms | **4.63 MiB** |
| mimalloc | 19.4 MiB | 20.1 / 40.2 / 28.2 / 26.2 | 62.3 MiB | 31 ms | 4.70 MiB |
| mimalloc, tuned | 12.8 MiB | 13.4 / 32.8 / 19.0 / 18.1 | 20.2 MiB | 32 ms | 4.70 MiB |
| jemalloc | 13.4 MiB | 14.1 / 27.7 / 15.1 / 14.3 | 35.6 MiB | 35 ms | 4.88 MiB |
| jemalloc, tuned | 10.0 MiB | 12.0 / 22.2 / 14.6 / 14.0 | 10.3 MiB | 42 ms | 4.88 MiB |
| glibc malloc (`-gnu` build) | 6.7 MiB | 8.0 / 18.7 / 12.5 / 10.7 | 19.3 MiB | 35 ms | 4.88 MiB |

Memory comes first for dasher, and musl's allocator wins on every memory
figure. Both alternatives start with more memory reserved than a whole
dasher job needs, and they hold on to freed memory for speed. musl's
`malloc` is the slowest of the three, but that barely shows. Cold start
varied between runs: 37 to 62 ms with musl, against 31 to 42 ms for the
others. The 100-job run took about the same total time with every
allocator (7 to 9 s), because page loads wait on the network and on
settling more than on `malloc`. Neither gain is worth 3 to 5 times the
idle RSS.

## Per-page limits

Each page has two hard limits. Going over either one stops the page and
frees its document, and the operation fails with
`dasher::Error::LimitExceeded`. One runaway page therefore cannot take
down a process that is serving other jobs.

- **JS heap**: `BrowserBuilder::memory_limit`, 256 MiB by default
  (`--memory-limit` in the CLI). A refused allocation only throws inside
  the script, as in a browser. But if an allocation is refused while more
  than half the heap is in use, the page has filled its heap and is
  stopped.
- **DOM nodes**: `BrowserBuilder::max_dom_nodes`, 500,000 by default
  (`--max-dom-nodes`). The DOM lives outside the JS heap, so the heap limit
  cannot bound it. The `large` page's 75,000 nodes cost about 10 MiB, so
  the default caps a runaway document at roughly 65 MiB. That is far beyond
  real pages, which rarely pass 100,000 nodes. Fragment parsing and deep
  clones stop early once over the limit, so a single
  `innerHTML = hugeString` cannot overshoot it by much.
