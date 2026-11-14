# dasher-bench

Measures dasher's footprint (memory, time and binary size) on a fixed
fixture suite, and checks the results against [`budgets.json`](budgets.json).
CI runs it on every push, on the static musl release build, and fails if a
budget is exceeded. The results show up in the job summary and are
uploaded as the `benchmarks` artifact. [Micro-benchmarks](#micro-benchmarks)
are a separate mode, for the CPU cost of single language operations and
DOM calls.

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
event, then 500 ms of network idle), plus auto-scroll
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
  the glue compiled, but no page opened. Its script cache is empty then;
  a long-running process idles higher by what the cache holds, up to its
  bound (32 MiB by default; see [the script cache](#the-script-cache)).
- **sequential jobs**: one process, one browser, 100 jobs that cycle
  through the four pages. Each job runs on a new page that is dropped
  afterwards, and RSS is sampled after every job. The first 10 jobs are a
  warm-up while pools and caches fill. Growth is the highest RSS of the
  second half of the remaining jobs minus that of the first half. RSS
  swings by a few hundred KiB with the mix of pages, so comparing single
  samples would mostly measure where each one fell in the cycle. The
  browser keeps its default [script cache](#the-script-cache), as a
  service does, so the RSS between jobs includes what it holds; the
  report gives its size and hits. On the fixtures it holds the React
  builds, 0.22 MiB, which put the RSS between jobs 0.3 MiB above a run
  with the cache off (`DASHER_SCRIPT_CACHE_MB=0`; aarch64 Linux, glibc).
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
the OS when the page is dropped. QuickJS calls it directly, through
functions of dasher-js's own, not through rquickjs's allocator wrappers.
[Real pages](#real-pages) has what that bought: 22% less CPU time and 9% less peak RSS than `malloc` on real
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

Set `DASHER_REPLAY_LOG` as well to have every request the replay answers
(or refuses) printed to stderr as `replay: hit|miss <method> <url>`, to
count a page's requests.

With `DASHER_EAGER_JS` set, a `replay` build compiles every function with
its script instead of on its first call (see
[`vendor/rquickjs-sys/PATCHES.md`](../../vendor/rquickjs-sys/PATCHES.md)),
so one build can be measured both ways.

Compare CPU time (user plus system), not wall time: under the virtual clock
a page's wall time is its CPU time plus fixed waits. Run each page three
times and take the median. Runs of one build agree to about 1%. On a
machine that is busy with other work they agree much less, so alternate
the two builds page by page rather than running one after the other.

Page output is not byte-for-byte the same from one run to the next, even
on one build: pages put timestamps and random IDs into it, and some insert
styles in an order that varies. Compare the element count, the count of
each tag, the link count and the visible text with digits masked.

### The parse-only corpus

How fast the engine compiles scripts is measured on the scripts of the
recorded pages, compiled without being run. Every response whose
`content-type` is JavaScript is a script (`*.meta` holds the headers and
`*.body` the bytes). Copy those bodies into one directory, drop
duplicates, and run

```sh
cargo run --profile profiling -p dasher-js --example compile_corpus -- corpus/
```

It compiles each file as a classic script, or as a module if that fails
(imports resolve to empty modules), in a page's runtime, and prints the
bytes compiled, the CPU time it took, the rate in MB/s and how much of the
JS heap the compiled scripts hold.

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
| functions compiled on their first call, not with their script (16 heavy pages) | +3% | −8% (−22% on one page) | Linux |
| compiled scripts kept across pages ([script cache](#the-script-cache); 16 heavy pages, third visit on) | −20% (−65% on one page) | +5%, the cache itself | Linux |

Before the allocator change, a quarter to over a third of CPU time on
the heavy pages was inside `malloc`, `free` and `malloc_usable_size`
(27% in a Linux profile of one page, 30 to 39% in macOS profiles of six).
QuickJS asks for the size of a block on every allocation and every free.

Tried and left out, with what each measured:

- **mimalloc for the JS heap**, one heap per page: 12% less CPU on Linux
  but 20% more peak RSS.
- **QuickJS at `-O3`**: no faster than `-O2` on Linux, and larger.
- **A bytecode cache, as first tried** (compile each script once, keyed by
  its text): 24% less CPU with the cache warm, but 10% more memory,
  because functions read back from bytecode each carried their own source
  text. The [script cache](#the-script-cache) that replaced it shares the
  text.
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

### The script cache

Pages keep compiled scripts for later pages in a
[`ScriptCache`](../js/src/script_cache.rs) (its module docs say how). It
is measured with `dasher-bench pages`, which loads recorded pages one
after another in one process, with or without a cache, and prints a line
per load: CPU for the load and for dropping the page, peak RSS (reset
before each load), cache hits and misses, and the page's element count
and a hash of its text. Build it with the `replay` feature and point
`DASHER_REPLAY` at a directory holding every page's recording (files are
named by a hash of their URL, so recordings merge by copying them into
one directory):

```sh
cargo build --profile profiling -p dasher-bench --features replay
DASHER_REPLAY=rec/all target/profiling/dasher-bench pages --cache page --rounds 4 \
  verge=https://www.theverge.com/ bbc=https://www.bbc.com/news
```

**Revisiting a site.** Sixteen heavy pages, each loaded four times in a
process of its own with a cache of its own, against four loads without a
cache, the two alternating page by page (aarch64 Linux, glibc profiling
build, machine shared with other work, 2026-11-14). CPU seconds, median
where there are several loads. The first load compiles every script and
writes it out as compiled, its functions still waiting for their first
call. The second reads them back, compiles the functions it runs on
their first call as the first did, and when it is dropped compiles each
script again with those functions compiled in (the second figure). The
third and fourth find them compiled.

| page | CPU, no cache | 1st load | 2nd + rebuild | 3rd and 4th | peak RSS, no cache | 1st | 3rd and 4th | cache MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| theverge.com | 5.82 | 6.06 | 5.18 + 0.83 | 5.01 (−14%) | 159 | 159 | 161 | 19.0 |
| techcrunch.com | 5.02 | 5.03 | 4.59 + 0.51 | 4.42 (−12%) | 90 | 96 | 95 | 10.2 |
| wired.com | 4.92 | 5.07 | 3.84 + 1.41 | 4.25 (−14%) | 180 | 186 | 197 | 30.7 |
| cloud.google.com | 3.69 | 4.02 | 3.44 + 0.68 | 3.15 (−15%) | 76 | 79 | 83 | 13.8 |
| yahoo.com | 6.13 | 6.76 | 6.23 + 0.15 | 5.88 (−4%) | 36 | 35 | 37 | 3.4 |
| zillow.com | 3.32 | 3.28 | 2.53 + 0.97 | 2.06 (−38%) | 109 | 108 | 117 | 20.3 |
| nytimes.com | 1.95 | 2.09 | 1.44 + 0.57 | 1.23 (−37%) | 121 | 122 | 129 | 16.1 |
| bbc.com/news | 1.81 | 1.79 | 1.49 + 0.24 | 1.44 (−20%) | 48 | 49 | 52 | 5.6 |
| airbnb.com | 2.48 | 2.32 | 1.98 + 0.43 | 1.81 (−27%) | 113 | 112 | 112 | 11.9 |
| timesofindia.indiatimes.com | 1.01 | 1.06 | 0.78 + 0.34 | 0.62 (−39%) | 49 | 51 | 51 | 7.2 |
| github.com (a repository) | 0.98 | 0.99 | 0.74 + 0.33 | 0.61 (−38%) | 49 | 49 | 52 | 6.8 |
| espn.com | 0.65 | 0.70 | 0.41 + 0.28 | 0.30 (−54%) | 50 | 46 | 47 | 8.2 |
| edition.cnn.com | 0.51 | 0.57 | 0.35 + 0.20 | 0.18 (−65%) | 60 | 58 | 62 | 7.5 |
| angular.dev | 0.34 | 0.41 | 0.23 + 0.15 | 0.19 (−45%) | 43 | 44 | 43 | 4.1 |
| react.dev | 0.32 | 0.35 | 0.23 + 0.11 | 0.19 (−41%) | 27 | 27 | 28 | 2.4 |
| theguardian.com | 0.04 | 0.08 | 0.03 + 0.01 | 0.03 (−35%) | 17 | 17 | 18 | 0.4 |
| **all sixteen** | **39.0** | **40.6** (+4%) | **40.7** (+4%) | **31.3** (−20%) | **1226** | **1237** (+1%) | **1283** (+5%) | **168** |

Two of airbnb.com's loads without a cache rendered a third of the page
(955 elements against 2,912) and are left out. The page text (digits
masked) was otherwise the same with and without the cache on every page;
element counts moved by a few on four pages, as they do from one load to
the next without the cache.

- **CPU.** From the third visit on, 20% less CPU over the sixteen pages,
  12 to 65% per page: what compiling cost, less reading the entries back
  (under 1% of a warm load's cycles in a profile of bbc.com). The first
  visit costs 0 to 4% more, for hashing each script, writing it out and
  compressing the code; on the cross-site run below, where nearly every
  script missed, 0.4 to 2%. The second visit pays for compiling the
  scripts again when the page is dropped, about what the third saves.
- **Memory.** The JS heap of a page using the cache is smaller by its
  scripts' text, which stays in the cache and is shared by every page
  using it. What the cache adds is the compiled code, compressed with
  zstd (about half). Peak RSS was 1% higher on the first visit and 5%
  higher from the third, with a cache that holds every script of the
  site: 168 MiB of entries over the sixteen sites, 0.4 to 31 MiB each.
  wired.com's scripts fill the default 32 MiB, so some are evicted on
  every visit.

**From disk.** `dasher --script-cache-dir <dir> <url>`, a new process each
time, as on AWS Lambda or a CLI run from a script. Eight pages, each run
twice to fill its directory, then three runs with the directory and three
without, alternating (CPU seconds and peak RSS in MiB, median):

| page | CPU, no cache | 1st run | 2nd run | with the directory | peak RSS, no cache | with |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| theverge.com | 5.80 | 6.02 | 6.22 | 4.85 (−16%) | 160 | 158 |
| zillow.com | 2.86 | 2.97 | 3.23 | 1.84 (−36%) | 107 | 114 |
| nytimes.com | 1.79 | 1.88 | 1.96 | 1.21 (−32%) | 123 | 127 |
| bbc.com/news | 1.77 | 1.81 | 1.86 | 1.48 (−16%) | 47 | 50 |
| timesofindia.indiatimes.com | 0.95 | 1.04 | 1.10 | 0.61 (−36%) | 48 | 51 |
| github.com (a repository) | 0.97 | 0.92 | 1.11 | 0.63 (−35%) | 48 | 50 |
| espn.com | 0.62 | 0.66 | 0.72 | 0.32 (−48%) | 46 | 46 |
| edition.cnn.com | 0.53 | 0.55 | 0.58 | 0.34 (−36%) | 60 | 58 |
| **all eight** | **15.3** | **15.9** (+4%) | **16.8** (+10%) | **11.3** (−26%) | **639** | **654** (+2%) |

A warm process reads every script from the directory (5 to 21 MB of
files per site); its cache holds only its one page's entries, whose text
the page would have held anyway. The second run, the one that compiles the scripts again, also
frees its page before exiting (the CLI otherwise leaves that to the OS,
see `crates/cli/src/run.rs`). Lightpanda's code cache on disk gave 8% on
a repeat visit.

**Across sites.** All 22 recorded sites (the sixteen and six lighter
ones) in one process, one after another, then all again, with one shared
cache, against the same without one:

| cache | script bytes found compiled, 1st pass (from other sites) | 2nd pass | CPU, 1st pass / 2nd | RSS |
| --- | ---: | ---: | ---: | --- |
| 32 MiB (default) | 3.8% | 3.8% | +0.4% / +1.8% | +32 to 36 MiB on every page |
| 128 MiB | 7.8% | 12.3% | +2.1% / −0.8% | up to +130 MiB |

Different sites share little: 30 of 1,069 scripts on the first pass (GTM
loaders, consent and analytics libraries), 5 MB of 126 MB. The 22 sites'
scripts are four times the default cache, so going round them again
evicts each before it comes back, and even 128 MiB holds a fraction. The
cache pays when the same site comes back while its scripts are still
held: a scraper working through one site, or a service whose jobs keep
to a few.

### Allocation and garbage collection

Profiled on 2026-11-13 at `babeb38` with `perf` (call graphs from DWARF
unwinding, 300 samples a second) on 13 heavy recorded pages: theverge.com,
techcrunch.com, wired.com, cloud.google.com, yahoo.com, zillow.com,
nytimes.com, bbc.com/news, airbnb.com, timesofindia.indiatimes.com,
github.com, espn.com and edition.cnn.com. The build was the static musl
`profiling` build on aarch64 Linux (AWS Graviton). A sample counts as
allocation when it is in the allocator or its wrappers, as collection when
the cycle collector is on the stack, and as freeing when it is in
QuickJS's code that releases values and frees objects. Each is charged to
the first caller outside that machinery:

| what | share of CPU | asked for by |
| --- | ---: | --- |
| allocating and freeing blocks | 12.8% | interpreter and built-ins 8.7%, parser and compiler 2.9%, host code (DOM bindings, rquickjs glue) 1.1% |
| the cycle collector | 5.4% | the trigger in `JS_NewObjectFromShape`, so whatever allocates objects |
| releasing values and freeing objects | 5.4% | the interpreter 4.9%, host 0.4%, parser 0.3% |
| the kernel (page faults, `mmap`) | 3.4% | |

What that showed:

- **Half of the allocator's time was calls, not allocating.** Each request
  went through `js_malloc_rt`, rquickjs's C wrapper, a `dyn Allocator`
  call and the slab heap, and asked for the block's size twice. What is
  left is memory latency: the slab fast path stalls on reading the link of
  a block it pops from a free list, and on fresh memory.
- **Every value release was a function call.** `JS_FreeValue` and
  `JS_FreeValueRT` were 4% of CPU in their own code: they are exported,
  and an exported function is not inlined under `-fPIC`.
- **The cycle collector is bound by memory latency.** Late in a heavy
  page a pass visits about 500,000 objects and 1.5 million properties, in
  50 to 100 ms (wired.com: 14 passes, 290 ms). Most passes on a growing
  heap free little: on 12 of the 13 pages most passes free under 10%.
  But those passes come at small heap sizes and are cheap: all of them
  together are about 490 ms of the 2.1 s the 13 pages spend collecting,
  1.2% of CPU.
- **Objects grow their shapes one property at a time.** `resize_properties`
  runs 1.8 million times on theverge.com, 1.2 million on wired.com and
  2.6 million on yahoo.com, almost all for plain objects and arrays of 2
  to 13 properties. A new object clones the empty shape and grows that
  private shape in place, with a resized property array at 2, 3, 4, 6, 9
  and 13 slots, until it reaches a shape some live object already has.
  Together with cloning shapes this is about 1.5% of CPU. Object literals
  made from a shape remembered at the creation site (DAS-36) remove it for
  literals.
- **The DOM boundary allocates little.** All allocation and freeing that
  host code asks for, the DOM bindings' argument and result conversions
  among it, is 1.5% of CPU.
- **Scripts are copied on their way into the engine.** The decoded text
  is copied out of the response, and rquickjs copies it again to add a
  terminating NUL. On wired.com those copies are 9% of the page faults,
  for about 0.3% of CPU. The copies are freed before the heap reaches its
  peak.

Changes made (PATCHES.md has each one): QuickJS calls the slab heap
through dasher-js's own functions (`JS_SetMallocFunctions`), with its fast
paths inlined and no ordered loads; the reference count tests of
`JS_FreeValue` and the atom functions are inlined in quickjs.c; and the
cycle collector marks children by direct, inlined steps. Measured
together against `main` at `dd42aac` (lazy compilation included) on the
same pages, five runs, median, with the two builds alternating:

| page | CPU before | after | | peak RSS before | after |
| --- | ---: | ---: | ---: | ---: | ---: |
| theverge.com | 6.46 s | 6.00 s | −7.1% | 153 MiB | 154 MiB |
| techcrunch.com | 5.36 | 5.00 | −6.7% | 87 | 87 |
| wired.com | 5.21 | 4.94 | −5.2% | 174 | 174 |
| cloud.google.com | 3.84 | 3.58 | −6.8% | 73 | 71 |
| zillow.com | 3.11 | 2.90 | −6.8% | 104 | 103 |
| nytimes.com | 1.94 | 1.87 | −3.6% | 105 | 105 |
| bbc.com/news | 1.94 | 1.80 | −7.2% | 44 | 44 |
| airbnb.com | 2.66 | 2.51 | −5.6% | 117 | 117 |
| timesofindia.indiatimes.com | 1.09 | 1.03 | −5.5% | 44 | 45 |
| github.com | 1.10 | 1.05 | −4.5% | 45 | 45 |
| espn.com | 0.69 | 0.66 | −4.3% | 41 | 41 |
| edition.cnn.com | 0.62 | 0.61 | −1.6% | 46 | 46 |
| **all twelve** | **34.02 s** | **31.95 s** | **−6.1%** | **1031 MiB** | **1031 MiB** |

Instructions fall by 5.7% and cycles by 6.2%. Each change on its own,
against the build before it: calling the heap directly 0.9% fewer cycles,
and 1.5% more with its fast paths inlined; inlined reference counts 4.0%;
the collector's direct marking 1.2%. yahoo.com is left out of the
table because its CPU time follows the clock: the same build varies by
15 to 20% between runs. The release binary grows by 70 KiB (5.69 to 5.76
MiB) and idle RSS by 68 KiB (4.10 to 4.17 MiB), the extra text the
process touches. The fixtures' peak RSS stays within 0.1 MiB.

Tried and left out:

- **Prefetching the next free block** when the slab heap pops one, so the
  next allocation does not stall on its link: 0.8% more cycles.
- **Collecting later after a pass that frees little**: letting the heap
  double before the next pass, instead of growing by half, whenever a
  pass freed under 10%. 0.9% fewer cycles, but 4 MiB more peak RSS summed
  over the twelve pages, and 3 to 5 MiB more on wired.com, zillow.com and
  nytimes.com. That trades memory for CPU, as running the collector less
  often did above.
- **Inlining only the tag test of `JS_FreeValue`**, with the decrement
  out of line, to save code: 22 KB less text than full inlining, but 2.8%
  more cycles, three quarters of the gain.

### Visibility

What counts as in view (the `--visibility` policy; see the main README)
decides how much lazy content a page loads. Sixteen pages were recorded
on 2026-11-13, each once under `all` and once under `viewport` into one
directory, and replayed on an aarch64 Linux profiling build (8 cores shared
with other work, 4 runs at a time): CPU seconds, requests and words of
`document.body.innerText`, median of 3, with a 60 s limit.

| page | `loaded` (default) CPU | requests | words | `all` CPU | requests | words | `viewport` CPU | requests | words |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| engadget.com | 1.73 | 165 | 4,128 | 43.94 | 5,587 | 18,680 | 1.72 | 114 | 1,615 |
| shopify.com | 3.25 | 168 | 1,052 | 3.18 | 169 | 1,052 | 1.47 | 140 | 1,052 |
| theverge.com | 7.26 | 229 | 3,753 | 6.91 | 228 | 3,753 | 4.94 | 159 | 3,753 |
| techcrunch.com | 6.21 | 221 | 1,841 | 5.99 | 220 | 1,841 | 5.32 | 97 | 1,841 |
| wired.com | 5.36 | 154 | 1,917 | 5.33 | 154 | 1,917 | 5.41 | 150 | 1,917 |
| zillow.com | 3.28 | 137 | 1,174 | 2.96 | 134 | 1,174 | 3.07 | 134 | 1,174 |
| cloud.google.com (a docs page) | 2.42 | 59 | 4,283 | 1.72 | 58 | 4,283 | 5.09 | 132 | 4,283 |
| bbc.com/news | 1.86 | 127 | 1,699 | 1.98 | 127 | 1,699 | 1.73 | 74 | 1,699 |
| timesofindia.indiatimes.com | 1.18 | 106 | 4,932 | 1.16 | 106 | 4,932 | 1.44 | 106 | 4,932 |
| github.com (a repository) | 1.09 | 103 | 1,108 | 1.05 | 103 | 1,108 | 1.07 | 103 | 1,108 |
| espn.com | 0.70 | 42 | 1,635 | 0.69 | 42 | 1,635 | 0.72 | 42 | 1,635 |
| edition.cnn.com | 0.60 | 19 | 2,079 | 0.58 | 19 | 2,079 | 0.60 | 19 | 2,079 |
| angular.dev | 0.37 | 39 | 368 | 0.37 | 39 | 368 | 0.37 | 39 | 368 |
| react.dev | 0.36 | 37 | 1,223 | 0.34 | 37 | 1,223 | 0.33 | 17 | 1,223 |
| developer.mozilla.org | 0.17 | 40 | 887 | 0.16 | 40 | 887 | 0.17 | 40 | 887 |
| en.wikipedia.org (an article) | 0.16 | 8 | 3,613 | 0.16 | 8 | 3,613 | 0.16 | 8 | 3,613 |
| **all sixteen** | **36.00** | **1,654** | **35,692** | **76.52** | **7,071** | **50,244** | **33.61** | **1,374** | **33,179** |

Before `loaded` existed the default was `all`; on the same recordings
the build before it (`babeb38`) gave 93.3 s and 9,481 requests under
`all`, and 32.7 s and 1,390 under `viewport`.

What it showed:

- **engadget.com: confirmed, and the trigger is position, not new
  elements.** Its loader (`loopin.min.js`) calls `loadMore()` again after
  every batch it appends, and each time checks
  `document.querySelector('.loading-more').getBoundingClientRect().top - 200
  <= scrollY + innerHeight`. `.loading-more` is in the document at load.
  Under `all` every box sits at the top of the viewport, so the check is
  always true and the loader fetches `?action=more-stories` until the time
  limit. In a browser the appended stories push `.loading-more` below the
  fold. `loaded` does the same: content inserted since the last reveal takes
  room, so after a batch or two the element leaves the viewport and the chain
  ends. Its run time varied under `all` (37.7 to 46.6 s, 4,815 to 6,021
  requests) as it filled the limit. Under `all` the extra words are the stories it kept
  loading. `loaded` with `--scroll` gets them all (18,680 words, the
  recording's whole supply) in 10.3 CPU seconds and 1,419 requests.
- **shopify.com: refuted.** Its lazy sections are present at load, and
  blocking the tracking scripts shows they are cheap. With Google Tag Manager
  (served from `gtm.shopify.com`) blocked, the page costs 0.41 to 0.43 s
  under every policy and keeps the same 1,052 words. With it, `all` and
  `loaded` cost 3.2 s and `viewport` 1.5 s. Of the 27 requests `all` makes
  beyond `viewport`, 26 are analytics beacons (`google.com/ccm/collect` and
  `gtm.shopify.com/g/collect`), sent when tagged elements come into view;
  blocking the beacons alone gives 1.7 s. So the cost is Google Tag
  Manager's own JavaScript, which visibility only sets going more often.
  The text is the same either way.
- **Text kept.** All three policies kept the same text on fifteen pages;
  only engadget differs, as above.
- **Elsewhere the policies cost the same**, within the noise of this
  machine. cloud.google.com has two outcomes, depending on an experiment
  the page draws at random. One makes 58 requests in about 2 s, the other
  132 requests (24 of them ad pings missing from the recording) in about
  4 to 5 s. Each policy happened to land on a different one, so its row
  says nothing about the policies.
- **What `loaded` costs itself**: working out where appended content sits
  is a walk of the document per geometry query once anything has been
  inserted since the last reveal. On theverge.com, the heaviest user of
  the four pages checked, it came to 4,000 calls and 85 ms of 7 s. A paired
  rerun of `loaded` and `all`, four runs each, gave the same CPU on
  zillow.com and 2% more on theverge.com, within the spread of either.

### Bounded page loads

An audit of `babeb38` found most of a sample's CPU time going to a few
pages that ran to the time limit at full CPU. Their DOM changes on a
timer forever, so the default wait (then network idle plus DOM quiet)
never held, and the virtual clock, skipping from one tick to the next,
turned 20 ms of work a second into a busy loop. Three budgets now bound
every load (see the main README): **page time** spent waiting for quiet
(5 s; skipped time, and real time the page sits idle, but not time
running scripts or waiting on the network), **CPU** (10 s) and
**requests** (1000). When one runs out the page is returned as it
stands, with `budget_exhausted` saying which. `requestIdleCallback` work
no longer holds a page open. While settling, the event loop now hands
control back once page time passes the next look, even if work is
always due, and a page with work due does not count as quiet.

142 sites were recorded on 2026-11-13 with `babeb38` (`-t 60`, the
default wait) and replayed on an aarch64 Linux profiling build, 6 at a
time, with other work sharing the 8 cores. The `babeb38` column is that
build; the others are `babeb38` plus this change, one run each (two
where two are given). A page **kept its text** when its visible words
(outside `script`, `style`, `svg` and `template`) include at least 90% of
those `babeb38` produced, counted with repeats. Three pages crashed
(`use of a released node`, a separate bug in reclaiming script elements
a page removes before the loader is done with them; the change makes it
show more often) and count as lost in every run of this change. With
main merged, which fixes it, they load and settle (cnet.com 1.7 CPU s,
expedia.com 0.16, hotels.com 0.4), as do huffpost.com and zdnet.com. Six
more sites (washingtonpost.com, bestbuy.com and costco.com would not
load, huffpost.com, zdnet.com and ign.com crashed in `babeb38`) are not
in the 142.

| wait, page-time budget | CPU, all 142 | text kept | timed out | ended by a budget |
| --- | ---: | ---: | ---: | --- |
| `babeb38`, settled, no budgets | 844.2 s | (142) | 12 | |
| settled, 3 s | 211.8 s | 137 | 0 | page time 28, CPU 2 |
| settled, 5 s | 234.3, 237.1 s | 136, 134 | 0 | page time 22–25, CPU 2–3, requests 0–1 |
| settled, 10 s | 248.2 s | 137 | 0 | page time 18, CPU 3 |
| **network idle, 5 s (the new default)** | **219.8, 229.1 s** | **138, 137** | **0** | **page time 10–15, CPU 2–3** |

The ticket left a choice: change the default wait to network idle, or
keep `settled` and give up DOM quiet once the budget is spent (which is
what `settled` with a page-time budget does). Network idle kept as much
text or more for less CPU, so it is the default now; `--wait settled`,
`"wait": "settled"` and `SettleOptions::dom_quiet` still wait for both.
The difference is small, within the run-to-run spread of about two pages
on rotating content (soundcloud.com's front page changes as it plays).
Network idle loses some text on two pages that render after the network
goes quiet (cnbc.com, airbnb.com in one run of two); `settled` loses it on
the same two. An earlier version that counted all real page time,
running scripts included, cost 180, 206 and 241 s at 2, 5 and 10 s and
kept 128, 132 and 136 pages. It cut heavy pages short whenever the
machine was busy, which tied the budget to the speed of the machine.

The pages that spun, CPU seconds, median of 3 runs, 4 at a time. The
`babeb38` runs are lower than 60 s because the machine was shared, which
held each spinning page below a full core; the ratios hold.

| page | `babeb38` | this change | with main merged (DAS-31's `loaded` visibility and others) | ended by |
| --- | ---: | ---: | ---: | --- |
| soundcloud.com | 26.21 (timed out) | 2.65 | 2.36 | settled |
| cloudflare.com | 22.80 (timed out) | 0.85 | 0.77 | settled |
| engadget.com | 30.18 (timed out) | 4.98 | 1.64 | requests, then settled once `loaded` stops its loader |
| salesforce.com | 30.70 (timed out) | 10.01 | 10.01 | CPU budget |
| archive.org | 28.68 (timed out) | 3.74 | 0.17 | page time or CPU, then settled |
| figma.com | 25.43 (timed out) | 2.19 | 2.35 | settled |
| microsoft.com/en-us | 0.06 | 0.07 | 0.06 | settled (not a spinner in this recording) |

On the full corpus, run 6 at a time with nothing else, the other pages
that had spun (hubspot.com, docker.com, samsung.com, twilio.com, hp.com,
squarespace.com, 45 to 51 s each) took 1.3 to 3.5 s, except hubspot.com
at the 10 s CPU budget.

Two pages still end on the CPU budget, and neither is a timer problem.
salesforce.com runs one task that never yields, inside Google Tag
Manager's macro interpreter (`gtm.js`, recursing through `to` and
`evaluate`). hubspot.com spends its time in a MutationObserver callback
that copies huge NodeLists over and over. In both, the time goes to the
page's own scripts, which only the CPU budget bounds. Most of the
1000-request budget is headroom: the heaviest page that settled made 409
requests (nytimes.com). Wall time is not compared: with six pages
replaying at once, contention more than doubled it on heavy pages. Run
one at a time, wall times matched `babeb38` or beat it (bloomberg.com
9.5 s against 10.8 s, dell.com 4.2 against 5.4).

## Micro-benchmarks

`dasher-bench micro` times small fixed loops of one language operation or
one DOM call each, and prints the CPU time per operation. Engine and DOM
work measures itself with it: run it before and after a change, on the same
machine. It is not run or budgeted in CI, because the numbers depend on the
runner.

```sh
cargo build --release -p dasher-bench
target/release/dasher-bench micro                     # both suites, about 50 s
target/release/dasher-bench micro --filter dom        # one suite
target/release/dasher-bench micro --filter shape      # names containing "shape"
target/release/dasher-bench micro --runs 9 --json micro.json
```

The loops are in [`micro/language.js`](micro/language.js) and
[`micro/dom.js`](micro/dom.js), one `bench(name, iterations, fn)` call each.
Each benchmark is a page load, in a new page of one browser. The language
suite's page is empty. The DOM suite's page is about 9,000 nodes (9,020):
500 cards of a heading, a paragraph and a list of four links, with the
`#target` element the calls go to in the middle and `#last` at the end. A
query string tells the suite's script which loop to run. The same page with
no loop to run is the baseline. The cost of a benchmark is the median CPU
time of its loads less the median of the baseline's, divided by its
iterations. CPU time is that of the thread running the page, so the
in-process fixture server does not count. Before timing, a loop that takes
under 100 ms gets more iterations in proportion, so its time stays well
above the baseline's jitter as dasher gets faster. That leaves ns per op
comparable across builds. A loop body must not change, though: once it
does, old numbers no longer compare. Two full runs of one build agreed to
within 3% on all but one benchmark (innerText, 11%). Check a surprising
change with `--filter` and more `--runs`.

[`micro/node.js`](micro/node.js) runs the language loops under Node, sized
and timed the same way, for a V8 reference on the same machine:

```sh
node --jitless --single-threaded crates/bench/micro/node.js [filter]
```

`--jitless` takes away V8's compilers but keeps its interpreter's inline
caches. That is the fair comparison for page-load code: on real pages V8's
JIT tiers were worth nothing (0.98×). `--single-threaded` keeps GC on one
thread, as in dasher.

### Baseline

Release build (`aarch64-unknown-linux-gnu`) of commit `babeb38` on an AWS
Graviton4 (Neoverse-V2), 2026-11-13, median of 5 runs. Node 22.23
(`--jitless --single-threaded`) on the same machine.

The "audit" columns are from an earlier audit of the same commit against
V8 with its JIT off, and against Lightpanda for DOM calls. It ran on
another machine with loops of its own, so its seconds do not compare with
ns per op here. Compare the ratios instead. Where the two ratios differ by
a lot (regexps, object literals, for-in, map/filter/reduce, JSON), the loops here do a different
mix of work. Lightpanda does real layout, which is why its
`getBoundingClientRect` is slow.

| benchmark | dasher ns/op | Node `--jitless` ns/op | dasher ÷ Node | audit: dasher s | audit: V8 no JIT s | dasher ÷ V8 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| function call | 56.5 | 30.6 | 1.8× | 0.66 | 0.45 | 1.5× |
| method call | 80.5 | 37.8 | 2.1× | 0.71 | 0.62 | 1.1× |
| create+call closure | 179 | 95.9 | 1.9× | 0.61 | 0.28 | 2.2× |
| integer loop | 34.0 | 15.6 | 2.2× | 3.09 | 2.18 | 1.4× |
| property read/write, one shape | 58.0 | 26.3 | 2.2× | 1.99 | 1.31 | 1.5× |
| property read/write, four shapes | 73.6 | 33.2 | 2.2× | 1.46 | 1.10 | 1.3× |
| property read/write, 64 shapes | 69.7 | 35.5 | 2.0× | 1.16 | 0.77 | 1.5× |
| object literal | 264 | 33.1 | 8.0× | 0.94 | 0.27 | 3.5× |
| spread and Object.assign | 869 | 147 | 5.9× | 0.85 | 0.17 | 5.0× |
| destructuring and rest/spread | 1,242 | 342 | 3.6× | 2.08 | 0.68 | 3.1× |
| apply and arguments | 370 | 130 | 2.8× | 1.46 | 0.59 | 2.5× |
| map/filter/reduce | 2,133 | 670 | 3.2× | 0.26 | 0.20 | 1.3× |
| concat.apply | 828 | 198 | 4.2× | 0.62 | 0.25 | 2.5× |
| string concat | 153 | 44.0 | 3.5× | 0.19 | 0.05 | 3.8× |
| string methods | 1,471 | 564 | 2.6× | 0.29 | 0.16 | 1.8× |
| regexps | 6,146 | 510 | 12.1× | 0.42 | 0.11 | 3.8× |
| JSON | 4,337 | 1,178 | 3.7× | 0.29 | 0.04 | 7.2× |
| Map/Set | 267 | 103 | 2.6× | 0.60 | 0.44 | 1.4× |
| class instantiation | 264 | 94.2 | 2.8× | 0.87 | 0.44 | 2.0× |
| promise chain | 1,216 | 294 | 4.1× | 0.23 | 0.09 | 2.6× |
| async/await | 785 | 138 | 5.7× | 0.19 | 0.05 | 3.8× |
| generators | 64.1 | 83.3 | 0.8× | 0.09 | 0.17 | 0.5× |
| Date and Intl formatting | 47,141 | 2,532 | 18.6× | 1.14 | 0.14 | 8.1× |
| try/catch | 157 | 714 | 0.2× | 0.07 | 0.25 | 0.3× |
| for-in and Object.keys | 1,311 | 170 | 7.7× | 1.82 | 1.11 | 1.6× |
| URL parsing | 2,948 | 1,482 | 2.0× | 0.62 | 0.23 | 2.7× |
| new Function | 3,619 | 1,066 | 3.4× | 0.20 | 0.04 | 5.0× |

Since the baseline, DAS-39 moved Intl formatting and URL parsing into Rust
(`crates/js/src/intl/`, `crates/js/src/urls.rs`). The same machine, with
both builds and Node measured side by side and 9 runs each (the machine
was busy, so absolute figures run a little high):

| benchmark | before ns/op | after ns/op | Node `--jitless` ns/op | after ÷ Node |
| --- | ---: | ---: | ---: | ---: |
| Date and Intl formatting | 48,317 | 3,033 | 2,633 | 1.2× |
| URL parsing | 2,942 | 1,630 | 1,558 | 1.0× |

| benchmark | dasher ns/op | audit: dasher ns | audit: Lightpanda ns | dasher ÷ Lightpanda |
| --- | ---: | ---: | ---: | ---: |
| getElementById | 69,377 | 61,465 | 270 | 227.6× |
| getComputedStyle | 282 | 19,815 | 70 | 4.0× |
| classList.contains | 190 | 3,505 | 130 | 1.5× |
| style.setProperty | 753 | 3,432 | 303 | 2.5× |
| add+removeEventListener | 3,480 | 2,165 | 220 | 9.8× |
| clientWidth | 107 | 1,685 | 50 | 2.1× |
| append+removeChild | 1,934 | 1,315 | 220 | 6.0× |
| dataset.x | 276 | 1,198 | 137 | 2.0× |
| setAttribute | 1,231 | 865 | 170 | 5.1× |
| attributes.length | 1,319 | 798 | 70 | 11.4× |
| getAttribute | 715 | 465 | 120 | 3.9× |
| matches(tag) | 642 | 365 | 90 | 4.1× |
| querySelector(tag) | 710 | 432 | 103 | 4.2× |
| tagName | 97.0 | 175 | 60 | 1.6× |
| nextSibling | 90.7 | 165 | 50 | 1.8× |
| nodeType | 90.5 | 115 | 40 | 2.3× |
| shadowRoot | 79.5 | 115 | 40 | 2.0× |
| children | 357 | 215 | 350 | 0.6× |
| innerText | 374 | 215 | 220 | 1.0× |
| createElement | 1,607 | 915 | 470 | 1.9× |
| document.cookie | 503 | 165 | 320 | 0.5× |
| getBoundingClientRect | 1,095 | 1,615 | 24,770 | 0.04× |
| querySelectorAll(#id) | 116,395 | | | |
| querySelectorAll(.class) | 171,475 | | | |
| querySelectorAll(tag) | 269,684 | | | |

Most DOM calls here cost 1.5 to 2 times the audit's figures, ranging from
1.1 times for getElementById to 3 times for `document.cookie`. That fits a
slower machine and loops that differ in detail. The ratios to Lightpanda
are what to aim at.

tagName, nextSibling, nodeType and shadowRoot are native members (see
"Native members" in `crates/js/src/dom.rs`) since DAS-41; they were 329,
251, 197 and 209 ns here before. On this machine a native getter that does
nothing costs about 78 ns in this loop: 51 ns for the loop and the lookup
through four prototypes (an own data property is 40 ns), and 27 ns for
QuickJS calling a C function as a getter. The members' own work is the
rest, 0 to 20 ns.

Inline style, `classList`, `dataset`, `getComputedStyle` and the layout
getters are native since DAS-42 (see `crates/js/src/dom/host.rs`). Before,
getComputedStyle was 40,009 ns here, classList.contains 5,878,
style.setProperty 5,671, clientWidth 2,406, dataset.x 1,894 and
getBoundingClientRect 2,236. Where the time goes now:

- `getComputedStyle(el).display` is the native function returning the
  element's cached computed style (88 ns), then the property read, which
  QuickJS asks the object's exotic `get_own_property` for: converting the
  key, checking the prototype chain, finding the inline declaration and
  resolving the default (about 200 ns). Visibility (`:visible`, which walks
  the ancestors) and boxes are cached until the document next changes.
- `style.setProperty` includes building the value string in the loop
  (about 90 ns) and `el.style` (about 90 ns). Of the rest, much is the
  attribute write shared with `setAttribute` (mutation records, custom
  element reactions).
- clientWidth (107 ns) and dataset.x (276 ns, a property answered by the
  exotic method: one attribute lookup) are near the floor of a native
  getter described above.

### DOM indexes and selector plans

The id index in dasher-dom and the query plans in dasher-css (see their
crate docs) changed these rows. Same machine and build settings as the
baseline, `main` at `cfa9030` against the change, run back to back, median
of 5 runs, on a machine under load from other builds:

| benchmark | before ns/op | after ns/op | |
| --- | ---: | ---: | ---: |
| getElementById | 69,110 | 556 | 124× |
| querySelectorAll(#id) | 118,503 | 1,170 | 101× |
| querySelectorAll(.class) | 174,353 | 146,582 | 1.19× |
| querySelectorAll(tag) | 272,475 | 233,384 | 1.17× |
| matches(tag) | 672 | 625 | 1.08× |
| querySelector(tag) | 728 | 683 | 1.07× |

The other DOM rows did not change beyond noise; `setAttribute` and
`append+removeChild` pay nothing for keeping the index. Most of what is
left of these calls is the JS bindings: in Rust, without them,
`getElementById` takes 54 ns and `querySelector("#id")` 86 ns, against
57 µs and 108 µs before, and `matches("div")` 14 ns.
[`../css/examples/query_timing.rs`](../css/examples/query_timing.rs) times
queries at that level, on the same page:

```sh
cargo run --release -p dasher-css --example query_timing
```

It prints the best of 25 rounds per query, which stays steady on a busy
machine where the median does not. Selectors with an `#id` further left
(`main #c250 a`, `#target span`) went from 60 to 250 µs to 0.1 to 0.5 µs;
single-compound selectors (`.text`, `li`, `[data-x='3']`) became 1.3 to
1.5× faster; descendant and child selectors without an id (`div.a > p`,
`section.card h2`) stayed where they were.

The index costs 11.8 bytes per element with an id (`tests/memory.rs` in
dasher-dom), 1.7 bytes per node on its listing page where every ninth
node has an id. Peak RSS of the four fixture pages did not move beyond
run-to-run noise (±0.5 MiB). There is no class or tag index and no
ancestor filter: a tree walk costs about 10 ns per element, and an index
would cost every page memory for the few that query that way.

### Native events

Events are native since DAS-44 (see `crates/js/src/events/`): listener
storage, `Event` objects and dispatch are Rust, and the event path is
walked on node ids. On this machine, median of 5 runs, against
Lightpanda's nightly (1.1.0-nightly.12116) timed on the same loops with
[`micro/lightpanda.py`](micro/lightpanda.py):

| benchmark | before (main at `c13aea4`) | after | Lightpanda | after ÷ Lightpanda |
| --- | ---: | ---: | ---: | ---: |
| add+removeEventListener | 3,378 | 235 | 233 | 1.0× |
| dispatchEvent, bubbling 3 levels | 19,073 | 1,369 | 1,734 | 0.8× |
| tagName (for scale) | 97.3 | 95.3 | 72 | 1.3× |

The other DOM rows did not move beyond noise.

The dispatch loop makes a bubbling `Event` and dispatches it at a span
whose listener is three levels up, on the body: seven targets on the path
(span, `#target`, `main`, body, html, document, window), one of them with
a listener. Firing an event no listener could hear, as the host does for
`load`, `DOMContentLoaded`, script `load` and `error` or
`readystatechange` on most pages, makes no event object and no wrappers
and runs no JS: the path is checked for listeners, handlers and handler
attributes first.

`micro/lightpanda.py` serves the micro-benchmark page and loops to a
Lightpanda binary and reports ns per op the way `dasher-bench micro` does,
from the process CPU time of `lightpanda fetch` with and without a loop:

```sh
python3 -I crates/bench/micro/lightpanda.py path/to/lightpanda \
    "add+removeEventListener=500000" "dispatchEvent, bubbling 3 levels=200000"
```

Its tagName figure (72 ns here, 60 in the audit) puts this machine about
1.2 times slower than the audit's for Lightpanda.

### Strings, atoms and JSON

In-place `+=`, a JSON scanner and writer of the engine's own, `memchr`
string search and no dump code in QuickJS (DAS-37, see
[`vendor/rquickjs-sys/PATCHES.md`](../../vendor/rquickjs-sys/PATCHES.md)).
Same machine, main at `045dfd2` with and without the changes, median of 5
runs, against Node 22 `--jitless --single-threaded`:

| benchmark | before ns/op | after ns/op | Node `--jitless` ns/op | after ÷ Node |
| --- | ---: | ---: | ---: | ---: |
| string concat | 160 | 77.3 | 44.3 | 1.7× |
| string methods | 1,632 | 928 | 561 | 1.7× |
| JSON | 4,538 | 1,487 | 1,174 | 1.3× |

The other language rows moved by 0 to 7%, most of them down, which is the
smaller heap and code layout rather than anything they call. What is left
of the first two is mostly the interpreter around the calls and
allocation; with DAS-36 and DAS-46 merged in as they stood, the three rows
came to 55.2, 730 and 1,248 ns. Parsing a 2.1 MB JSON array of 20,000
records takes 10 ms instead of 38 (Node `--jitless`: 62), and writing it
12 ms instead of 53 (16). On 14 heavy recorded pages: 3% less CPU time
and 1.1 to 1.3% less peak RSS; the per-change figures are in `PATCHES.md`.
### Language fast paths

DAS-36 added the fast paths in
[`vendor/rquickjs-sys/PATCHES.md`](../../vendor/rquickjs-sys/PATCHES.md)
("Fast paths for calls, object creation, copying, iteration and async",
and "`i++` on a `let` stays an int add"). Same machine and build settings
as the baseline; before is `main` at `6a9393e` with the engine source of
`cfa9030`, after is the change, run back to back, median of 5 runs, on a
machine under load from other builds. Node is the `--jitless` column of
the baseline table.

| benchmark | before ns/op | after ns/op | | after ÷ Node |
| --- | ---: | ---: | ---: | ---: |
| function call | 57.7 | 46.3 | 1.25× | 1.5× |
| create+call closure | 186 | 158 | 1.18× | 1.6× |
| integer loop | 36.0 | 25.1 | 1.43× | 1.6× |
| object literal | 267 | 111 | 2.41× | 3.4× |
| spread and Object.assign | 904 | 524 | 1.73× | 3.6× |
| destructuring and rest/spread | 1,284 | 877 | 1.46× | 2.6× |
| apply and arguments | 391 | 301 | 1.30× | 2.3× |
| map/filter/reduce | 2,247 | 1,696 | 1.32× | 2.5× |
| concat.apply | 865 | 518 | 1.67× | 2.6× |
| Map/Set | 284 | 224 | 1.27× | 2.2× |
| class instantiation | 288 | 276 | 1.04× | 2.9× |
| promise chain | 1,383 | 1,082 | 1.28× | 3.7× |
| async/await | 817 | 329 | 2.48× | 2.4× |
| for-in and Object.keys | 1,317 | 552 | 2.39× | 3.2× |
| tagName (a native getter) | 99.9 | 82.5 | 1.21× | |
| nodeType (a native getter) | 89.3 | 73.2 | 1.22× | |

Every other language row moved by 2 to 27%, from the cheaper `i++` and
C calls in their loops, and the DOM rows by 0 to 26%: a native getter or
method is now called without entering the interpreter. The aim was 1.3
times V8 without its JIT, and these stay 1.5 to 3.7 times. What is left
is mostly allocation and freeing of each object, property access through
the prototype chain (`this.x = …` in a constructor, a method looked up
on a prototype; DAS-32's inline caches) and garbage collection (the
promise chain keeps 100,000 promises alive; DAS-46).

The thirteen heavy recorded pages ([Real pages](#real-pages)), the same
two builds, five runs of each alternating, median CPU seconds and peak
RSS:

| page | CPU before | after | | peak RSS before | after |
| --- | ---: | ---: | ---: | ---: | ---: |
| theverge.com | 6.92 s | 6.43 s | −7.1% | 162.7 MiB | 162.1 MiB |
| techcrunch.com | 5.59 | 5.23 | −6.4% | 96.2 | 97.4 |
| wired.com | 5.12 | 4.82 | −5.9% | 188.9 | 190.2 |
| yahoo.com | 4.61 | 4.02 | −12.8% | 80.7 | 81.6 |
| cloud.google.com | 3.80 | 3.56 | −6.3% | 76.8 | 75.9 |
| zillow.com | 2.89 | 2.75 | −4.8% | 113.2 | 114.3 |
| airbnb.com | 2.57 | 2.41 | −6.2% | 124.8 | 125.5 |
| bbc.com/news | 1.95 | 1.75 | −10.3% | 51.2 | 52.6 |
| nytimes.com | 1.88 | 1.86 | −1.1% | 123.1 | 120.0 |
| timesofindia | 1.09 | 1.03 | −5.5% | 52.2 | 52.3 |
| github.com | 1.01 | 0.99 | −2.0% | 53.4 | 53.8 |
| espn.com | 0.68 | 0.66 | −2.9% | 53.3 | 53.1 |
| edition.cnn.com | 0.59 | 0.59 | 0% | 75.2 | 75.4 |
| **all thirteen** | **38.70 s** | **36.10 s** | **−6.7%** | **1252 MiB** | **1254 MiB** |

Every page rendered the same number of elements with both builds, apart
from cloud.google.com, whose count varies from run to run with either.
cloud.google.com's CPU time also falls into two modes, about 2.6 and 3.6
s, from run to run, so its row says little. Peak RSS moved within the
run-to-run spread of 1 to 3 MiB. A first version, which kept the shape
of every object literal site from its first run, cost zillow.com 3 to 4
MiB; sites now keep a shape only from their second run.

### Native attributes, edits, queries and lists

DAS-43 made the attribute, tree-edit, query and live-collection members
native members (see "Native members" in `crates/js/src/dom.rs`), and the
live `NodeList`, `HTMLCollection` and `NamedNodeMap` native exotic objects
instead of a `Proxy` over a native object. `main` at `914ea5a` against the
change, run back to back on the same machine (Graviton4, load average
about 11), median of 5 runs. The Lightpanda column is the nightly
`1.1.0-nightly.12116` on the same machine, timing the same loops on the
same page in its own `performance.now()` loop (best of 5, wall time), so it
compares roughly: dasher's figures include its interpreter's ~30 ns loop.

| benchmark | before ns/op | after ns/op | | Lightpanda ns/op | after ÷ Lightpanda |
| --- | ---: | ---: | ---: | ---: | ---: |
| append+removeChild | 1,935 | 569 | 3.4× | 208 | 2.7× |
| setAttribute | 1,236 | 256 | 4.8× | 258 | 1.0× |
| attributes.length | 1,335 | 149 | 9.0× | 96 | 1.6× |
| getAttribute | 709 | 170 | 4.2× | 125 | 1.4× |
| matches(tag) | 617 | 157 | 3.9× | 91 | 1.7× |
| querySelector(tag) | 685 | 172 | 4.0× | 122 | 1.4× |
| createElement | 1,656 | 567 | 2.9× | 737 | 0.8× |
| getElementById | 534 | 195 | 2.7× | 435 | 0.4× |
| querySelectorAll(#id) | 1,155 | 476 | 2.4× | 1,000 | 0.5× |
| querySelectorAll(.class) | 132,812 | 108,931 | 1.2× | 59,975 | 1.8× |
| querySelectorAll(tag) | 194,597 | 108,614 | 1.8× | | |
| children | 369 | 94 | 3.9× | 446 | 0.2× |
| children[i] | 626 | 78 | 8.0× | 78 | 1.0× |
| hidden | 309 | 101 | 3.1× | 49 | 2.1× |
| a.href | 1,453 | 1,424 | 1.0× | 887 | 1.6× |

The other DOM rows did not move beyond noise. Against the audit's
Lightpanda figures, scaled by this machine's ratio to the audit's
(1,935 / 1,315 for append+removeChild), every row of the ticket is within
2×; against Lightpanda measured here, append+removeChild is not (2.7×).
What is left of it is mostly bookkeeping around the tree edit, which
itself costs ~75 ns in dasher-dom: regrouping wrapper rings (each link is
an rquickjs `Node`, so every clone and drop is two reference counts and
every borrow three calls into QuickJS), and counting the change as DOM
activity (an ancestor walk and a clock read) on both calls. Ring links as
raw object pointers would take most of the first; that needs marking raw
values from `Trace`, which rquickjs does not expose. `a.href` pays for
`internals.baseURL()` (JS) and a `Url::join` per read.

New loops: `children[i]`, `hidden` and `a.href`.

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
