# Comparing dasher with Lightpanda

Tools for the end-to-end comparison in
[`../README.md`](../README.md#against-lightpanda): both browsers load the
same recorded sites, with their default settings, and each load's CPU time,
peak RSS and visible text are kept. Python 3 with `cryptography`, and a C
compiler for `rus.c`.

| file | what it does |
| --- | --- |
| `sites.txt` | the 224 sites, `name url` per line |
| `proxy.py` | an HTTPS proxy over dasher's recording format, for Lightpanda: answers from a recording (as `DASHER_REPLAY` does, misses fail as network errors), or with `--record` fetches what is missing and adds it. It terminates TLS with a certificate per host from its own CA (`--ca DIR`, created on first use), which Lightpanda is given with `--ca-cert` |
| `run.py` | runs both browsers on the sites, N rounds, alternating which goes first; one JSON per load (CPU, peak RSS, wall time, exit, words, dasher's `budget_exhausted` and request count) |
| `rus.c` | runs a command and reports its CPU and peak RSS; build it as `cc -O2 -o rus rus.c` here. (A process forked from the Python runner would report the runner's RSS as its peak.) |
| `text.py` | the visible text of an HTML document (shadow trees included), as lowercase words with digits masked |
| `report.py` | the comparison: classes, totals, ratios, budgets |
| `classify.py` | splits `perf script` samples into kinds of work |
| `errors.py` | loads every site once in dasher and tallies page errors by message |

```sh
cargo build --profile profiling -p dasher-cli --features replay
cc -O2 -o crates/bench/compare/rus crates/bench/compare/rus.c
C=crates/bench/compare
# Record: dasher first, then Lightpanda adds what it asks for beyond that.
python3 -I $C/run.py --sites $C/sites.txt --rec rec --out m/rec-d --runs 1 --jobs 12 --browsers dasher --record
python3 -I $C/run.py --sites $C/sites.txt --rec rec --out m/rec-lp --runs 1 --jobs 12 --browsers lp --record \
    --lp path/to/lightpanda
# Complete the recordings once the browsers have run on them: each fetches
# only what the recording lacks (POSTs included, for dasher).
DASHER_REPLAY_FILL=1 python3 -I $C/run.py --sites $C/sites.txt --rec rec --out m/fill-d --runs 1 --browsers dasher
python3 -I $C/run.py --sites $C/sites.txt --rec rec --out m/fill-lp --runs 1 --browsers lp --record --lp path/to/lightpanda
# Measure and report.
python3 -I $C/run.py --sites $C/sites.txt --rec rec --out m/run --runs 3 --jobs 4 --lp path/to/lightpanda
python3 -I $C/report.py m/run --rec rec --per-site
```

Lightpanda runs as `lightpanda fetch --dump html --wait-until networkidle`
(its other settings default: a 5 s wait cap, JIT on) through the proxy;
`LIGHTPANDA_DISABLE_TELEMETRY` is set. dasher runs with no options but
`--json`, replaying in-process, so it pays nothing for TLS or HTTP while
Lightpanda pays for both to a local proxy (about 1% of its CPU in the
profiles).

How a site is classed (from the audit of `babeb38`): a load is usable if
it exited 0 and kept at least 20 words that are not a bot wall's. Both
usable, with word-set overlap (shared distinct words over the smaller
set) of at least 0.5 and word counts within 1.67×: *comparable*. Both
usable otherwise: *thin* for the one with fewer words. One usable:
*fail* for the other. *Matched* pages are comparable, end on no budget,
and fetched the same JavaScript bytes within 10%; *heavy* ones took over
a CPU second in either browser.
