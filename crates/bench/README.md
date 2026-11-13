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
an Apple M2 Pro, 3 runs per page, 2026-11-11:

| page | time to settle | peak RSS | page RSS | JS heap | DOM nodes | HTML out |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| static | 11 ms | 6.7 MiB | 2.8 MiB | 1.9 MiB | 289 | 11 KiB |
| large | 58 ms | 17.1 MiB | 9.9 MiB | 1.9 MiB | 75,194 | 1698 KiB |
| spa | 80 ms | 11.1 MiB | 7.1 MiB | 5.5 MiB | 2,293 | 76 KiB |
| feed | 134 ms | 9.1 MiB | 4.5 MiB | 2.8 MiB | 8,830 | 271 KiB |

| measure | value |
| --- | ---: |
| idle RSS (ready, no page) | 3.9 MiB |
| 100 sequential jobs: highest RSS between jobs, first half → second half | 5.7 → 5.7 MiB (-0.01 MiB) |
| 100 sequential jobs: peak RSS, total time | 17.8 MiB, 7.8 s |
| process start (`dasher --version`) | 0.4 ms |
| cold start to static page loaded | 44.4 ms |
| `dasher` binary size (CLI, HTTP service and Lambda runtime) | 5.25 MiB |

The JS heap figures are a little higher than they were with `malloc`
(1.7, 1.7, 5.1 and 2.5 MiB the day before): blocks on the slab allocator
are rounded up to a size class, and the engine counts the rounded size.

On macOS (aarch64, release build) the same suite gives 7.9 MiB idle, peak
RSS of 11.4, 24.5, 16.1 and 13.7 MiB for the four pages, and a binary of
5.0 MiB. Those numbers also fit the budgets, so the check can be run
locally, but idle RSS is close to its budget there. macOS pages are 16 KiB,
so the slab allocator's first page per size class costs more than on
Linux: peak RSS on these four pages is 0.5 to 1.4 MiB above what `malloc`
gave on the same day, against 0.3 MiB at most on Linux.

### Targets

The initial targets were under 15 MB RSS idle and under 80 MB peak for a
typical SPA page. Both are met by a wide margin: 3.9 MiB idle, and
11.1 MiB peak for the React shop. The targets stay as the product
promise. The CI budgets in `budgets.json` sit much closer to the
measurements, at about 1.3 to 2 times each one, so a regression shows up
long before the targets are at risk:

| budget | value | measured |
| --- | ---: | ---: |
| idle RSS | 8 MiB | 3.9 MiB (6.7 MiB with glibc) |
| peak RSS: static, large, spa, feed | 12, 28, 20, 16 MiB | 6.7, 17.1, 11.1, 9.1 MiB |
| RSS growth over sequential jobs | 0.75 MiB | 0.00 to 0.07 MiB |
| binary size | 6 MiB | 5.25 MiB |

Times are reported but not budgeted, because they depend too much on the
runner.

Real SPAs are heavier than the fixture: bigger bundles and more data. The
fixture needs a 5 MiB JS heap and about 11 MiB of RSS in all, which leaves room
for several times its script and state within 80 MB. Pages that go far
beyond that are what the per-page limits below guard against.

## Allocator

Rust code in dasher uses the **system allocator**: in the static musl
build that ships, musl's own `malloc`. The JS heap does not. Each page's
QuickJS runtime allocates from a slab allocator of dasher's own
(`crates/js/src/slab.rs`), which gives all of a page's JS memory back to
the OS when the page is dropped. [Real pages](#real-pages) has what that
bought: 22% less CPU time and 9% less peak RSS than `malloc` on real
sites. The fixtures here are too small to show it, and their peak RSS
moved by 0.3 MiB at most.

Before that, general-purpose replacements for the whole process were
measured with the same suite, with QuickJS still on `malloc`. They were built with
`#[global_allocator]`, plus `malloc` overriding so that QuickJS's heap
moved with them: mimalloc 0.1 with `override`, and tikv-jemallocator 0.6
with `unprefixed_malloc_on_supported_platforms`. Each also ran tuned for
low RSS. For mimalloc that meant `MIMALLOC_PURGE_DELAY=0`,
`MIMALLOC_ARENA_EAGER_COMMIT=0` and `MIMALLOC_ALLOW_LARGE_OS_PAGES=0`. For
jemalloc it meant `MALLOC_CONF=narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0`.
Results, on aarch64 Linux, measured before the HTTP service joined the
binary. The service added 0.4 MiB to the musl build's size.

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

## Real pages

The fixtures are small and well behaved. What a change does on real sites
is measured by recording pages once and replaying them. Built with the
`replay` feature, dasher writes every response it receives to a directory
when `DASHER_RECORD` names one, and answers every request from a directory
when `DASHER_REPLAY` does. Two builds then get the same bytes, and nothing
depends on the network or on the site changing between runs.

```sh
cargo build --profile profiling -p dasher-cli --features replay
DASHER_RECORD=rec/verge target/profiling/dasher -t 60 https://www.theverge.com/ >/dev/null
DASHER_REPLAY=rec/verge /usr/bin/time -l target/profiling/dasher -t 60 https://www.theverge.com/ >/dev/null
```

Compare CPU time (user plus system), not wall time: under the virtual clock
a page's wall time is its CPU time plus fixed waits. Run each page three
times and take the median. Runs of one build agree to about 1%.

### What it showed

Fourteen pages recorded on 2026-11-10 and replayed on the static musl build
(`aarch64-unknown-linux-musl`, in Docker on an Apple M2 Pro; CPU seconds
and peak RSS, median of 3). "Before" is commit `8578929`. "Now" has the
JS heap on the slab allocator, QuickJS built at `-O2`, and the two engine
changes in [`vendor/rquickjs-sys/PATCHES.md`](../../vendor/rquickjs-sys/PATCHES.md).

| page | CPU before | now | | peak RSS before | now |
| --- | ---: | ---: | ---: | ---: | ---: |
| theverge.com | 14.74 s | 7.32 s | 2.0× | 203 MiB | 171 MiB |
| wired.com | 9.61 | 5.73 | 1.7× | 221 | 192 |
| zillow.com | 5.30 | 3.21 | 1.7× | 131 | 109 |
| bbc.com/news | 5.02 | 3.33 | 1.5× | 93 | 75 |
| cloud.google.com (a docs page) | 4.85 | 3.12 | 1.6× | 76 | 70 |
| techcrunch.com | 9.50 | 6.35 | 1.5× | 107 | 90 |
| timesofindia.indiatimes.com | 2.11 | 1.28 | 1.6× | 57 | 51 |
| espn.com | 1.30 | 0.87 | 1.5× | 57 | 51 |
| github.com (a repository) | 1.55 | 1.13 | 1.4× | 59 | 51 |
| edition.cnn.com | 1.07 | 0.76 | 1.4× | 70 | 62 |
| react.dev | 0.55 | 0.36 | 1.5× | 28 | 26 |
| angular.dev | 0.66 | 0.44 | 1.5× | 44 | 41 |
| en.wikipedia.org (an article) | 0.23 | 0.16 | 1.4× | 19 | 17 |
| developer.mozilla.org (a reference page) | 0.25 | 0.17 | 1.5× | 18 | 17 |
| **all fourteen** | **56.74 s** | **34.23 s** | **1.7×** | **1183 MiB** | **1023 MiB** |

The same comparison on macOS gives 47.9 s and 30.0 s.

The table was taken before the last change listed below, the parser's
bracket lookahead, which takes a further 4% off the total on Linux.

Each change was also measured on its own, against the build without it:

| change | CPU time | peak memory | measured on |
| --- | ---: | ---: | --- |
| JS heap on the slab allocator, not `malloc` | −22% | −9% | Linux |
| QuickJS at `-O2`, not `-Os` | −16% | none | Linux |
| one copy of each script's text | within 2% | −7% | macOS |
| `stack` written when first read | −3% (−14% on one page) | none | macOS |
| bracket lookahead answers remembered by the parser | −5% (−4% on Linux) | none | macOS and Linux |
| closures and `arguments` made from a known shape, regexp searches run by dasher, `Map` hash mixed (together) | −4% (−8% on one page) | none | macOS |

Before the allocator change, a quarter to over a third of CPU time on
the heavy pages was inside `malloc`, `free` and `malloc_usable_size`
(27% in a Linux profile of one page, 30 to 39% in macOS profiles of six).
QuickJS asks for the size of a block on every allocation and every free.

Tried and left out, with what each measured:

- **mimalloc for the JS heap**, one heap per page: 12% less CPU on Linux
  but 20% more peak RSS.
- **QuickJS at `-O3`**: no faster than `-O2` on Linux, and larger.
- **A bytecode cache** (compile each script once, keyed by its text): 24%
  less CPU with the cache warm, but 10% more memory as tried, because
  functions read back from bytecode each carry their own source text.
- **Timers under the virtual clock**: `--realtime` and `--wait load` cost
  the same CPU as the default. The time goes to scripts that run before
  `load`, not to spinning the clock.
- **Running the cycle collector less often**: it is 4% of CPU time.
  Letting the heap double between collections instead of growing by half
  saved 1% of CPU for 5% more memory, and letting it triple saved 2% for
  14% more.
- **Holding async and script-inserted scripts until the parser has
  finished**, so that none starts while the parser waits for the document
  or a blocking script: 11% less CPU over the replayed pages (24.8 to
  22.1 s), all of it on one page (4.37 to 1.85 s) and nothing on the
  other thirteen. On that page a session-recording script otherwise
  starts at the parser's first wait and watches the rest of the document
  arrive. Replay overstates it, because a replayed response arrives at
  once and so every async script has loaded by that first wait. Nine
  paired loads of the page from the live network gave 6 to 10% less CPU,
  apart from one where the build without the hold took 4.68 s against
  2.68 s. Page text, element count and link count were the same on all
  fourteen pages. Left out because one page gains and it changes when
  scripts run: a page that inserts a script and counts on it loading
  before a later blocking script would stop working. Holding the scripts
  for 20 ms only gave the same on replay and nothing once responses had
  60 ms of latency, and holding timers and promise jobs as well made no
  difference.
- **Keeping the shapes objects have just left**, so that objects built
  one after another share them: two thirds of property additions on four
  pages found no shape to reuse, but reusing them was 0.4% less CPU, which
  is within the noise.

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
  `innerHTML = hugeString` cannot overshoot it by much. The limit counts
  live nodes, and removed nodes are freed as soon as script can no longer
  reach them, also while the page is still parsing, so a page that keeps
  rewriting the same few nodes (a ticker, a list re-rendered) stays flat
  however long it runs (`crates/js/tests/churn.rs`).
