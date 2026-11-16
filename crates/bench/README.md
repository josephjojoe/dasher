# Benchmarks

Four ways of measuring dasher, from fixtures in CI to live sites on AWS.

| Tool | What it answers | Input |
| --- | --- | --- |
| [`dasher-bench`](#the-fixture-suite) | Has memory, start-up or binary size regressed? | Four fixture pages, compiled in |
| [`dasher-bench micro`](#micro-benchmarks) | What does one language operation or DOM call cost? | Fixed loops |
| [`dasher-bench pages`](#recorded-pages) | What does the script cache save over a run of pages? | Recordings of real pages |
| [`compare/`](compare/README.md) | dasher against Lightpanda on identical input | Recordings of 224 sites |
| [`lambda/`](lambda/README.md) | dasher against Lightpanda and Chromium, live, where it is deployed | 492 live sites on AWS Lambda |

The comparison a reader most likely wants is the last one: its results
are in [`lambda/README.md`](lambda/README.md).

## The fixture suite

```sh
cargo build --release -p dasher-cli -p dasher-bench
target/release/dasher-bench --budgets crates/bench/budgets.json
```

`dasher-bench` loads each fixture page in a fresh child process, so the
peak RSS it reports belongs to that page alone, then measures an idle
process, a hundred jobs in a row in one process, process start and the
binary's size. The fixtures and the data they fetch are compiled into
the binary, so it runs in a container without the source tree.

With `--budgets` it fails when a result is over a limit in
[`budgets.json`](budgets.json). CI runs it that way on the static
x86_64 musl build on every push, and puts the table in the job summary.

Results for 0.1.0, static musl build on aarch64 Linux (Docker on an
Apple M2 Pro):

| Page | Time to settle | Peak RSS | JS heap | DOM nodes |
| --- | ---: | ---: | ---: | ---: |
| **static**: 11 KB article, no scripts | 10 ms | 6.8 MiB | 1.8 MiB | 289 |
| **large**: 1.6 MB listing, 5,000-row table, no scripts | 63 ms | 17.2 MiB | 1.8 MiB | 75,194 |
| **spa**: React 18 shop, 100 products and 10 review lists from a JSON API | 62 ms | 11.0 MiB | 4.7 MiB | 2,291 |
| **feed**: infinite scroll, 20 pages of 20 posts behind an `IntersectionObserver` | 114 ms | 8.8 MiB | 2.1 MiB | 8,829 |

| Measure | Value |
| --- | ---: |
| Idle RSS (ready, no page) | 2.5 MiB |
| 100 jobs in a row: highest RSS between jobs, first half → second half | 6.5 → 6.3 MiB |
| 100 jobs in a row: peak RSS | 18.5 MiB |
| Process start (`dasher --version`) | 0.3 ms |
| Process start to static page loaded | 11 ms |
| Binary size | 6.01 MiB |

Memory on macOS reads differently (the allocator and page size differ),
so footprint is always measured on Linux.

## Micro-benchmarks

```sh
target/release/dasher-bench micro            # both suites
target/release/dasher-bench micro --filter regexps
```

Times small fixed loops of language operations
([`micro/language.js`](micro/language.js)) and DOM calls
([`micro/dom.js`](micro/dom.js)) inside dasher, and prints CPU time per
operation: the median of several loads, less the cost of loading the
same page without the loop.

For the same loops elsewhere, on the same machine:

```sh
node --jitless --single-threaded crates/bench/micro/node.js      # V8's interpreter
python3 -I crates/bench/micro/lightpanda.py path/to/lightpanda --from micro.json
```

## Recorded pages

A real page asks for different work on every load: the site changes, an
ad auction goes another way, the network is slower. To compare two
builds on the same input, dasher can record a page's responses and
answer from the recording. This is the `replay` feature, off by default:

```sh
cargo build --profile profiling -p dasher-cli -p dasher-bench --features replay
DASHER_RECORD=rec/bbc target/profiling/dasher https://www.bbc.com/news > /dev/null
DASHER_REPLAY=rec/bbc target/profiling/dasher https://www.bbc.com/news > /dev/null
```

Under `DASHER_REPLAY` no request leaves the process, and one that was
never recorded fails like a refused connection. `DASHER_REPLAY_FILL=1`
fetches what is missing and adds it; `DASHER_REPLAY_LOG=1` prints each
request and whether the recording had it. Requests match by method and
URL, then by method, host and path, since pages put the clock and
random numbers in their URLs.

Two cautions when comparing builds on a recording:

- Check for misses first. A build that asks for something the recording
  lacks gets a network error where the other got a script, and the two
  are then doing different work.
- A recording goes stale by the clock: pages compare dates in their
  data with `Date.now()`, and can take another path minutes later.

`dasher-bench pages name=url ...` loads recorded pages one after another
in one process and prints a line per load (CPU, peak RSS, script cache
hits and misses, a hash of the page's text), for what the script cache
saves across pages and visits. See `dasher-bench pages --help`.
