#!/usr/bin/env python3
"""Times the DOM micro-benchmark loops under Lightpanda, for comparison.

It serves the same page and loops as `dasher-bench micro` (the page from
`dom_page` and `RUNNER` in src/micro.rs, the loops in micro/dom.js), loads it
with `lightpanda fetch`, with and without a loop, and reports the process CPU
time (user + system) of the median load with the loop, less the median
without, per iteration. Pick iteration counts that make each loop take a few
hundred milliseconds, so the process start-up's jitter stays small beside
it.

    python3 -I crates/bench/micro/lightpanda.py path/to/lightpanda \
        "add+removeEventListener=500000" "tagName=2000000" [--runs 5]
"""

import functools
import http.server
import os
import resource
import statistics
import subprocess
import sys
import tempfile
import threading
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.dirname(HERE)


def page():
    src = open(os.path.join(BENCH, "src", "micro.rs")).read()
    runner = src.split('const RUNNER: &str = r#"', 1)[1].split('"#;', 1)[0]
    style = src.split('const DOM_STYLE: &str = "', 1)[1].split('";', 1)[0]
    style = style.replace('\\"', '"').replace("\\\n", "").replace("  ", "")
    cards = []
    for i in range(500):
        if i == 250:
            cards.append(
                '<div id="target" class="a b c" data-x="1" style="color: red" title="t">'
                '<span>s</span><p id="para">Hello world</p></div>'
            )
        cards.append(
            f'<section class="card" id="c{i}" data-x="{i}"><h2>Card {i}</h2>'
            f'<p class="text">Text of card {i}</p><ul>'
        )
        cards.extend(f'<li><a href="/c/{i}/{j}">Link {j}</a></li>' for j in range(4))
        cards.append("</ul></section>")
    body = "<main>" + "".join(cards) + '<div id="last"></div></main>'
    return (
        '<!doctype html><html><head><meta charset="utf-8"><title>dom</title>'
        + style + "</head><body>" + body
        + '<script src="/micro/dom.js"></script><script>' + runner
        + "</script></body></html>"
    )


def main(argv):
    runs = 5
    if "--runs" in argv:
        i = argv.index("--runs")
        runs = int(argv[i + 1])
        del argv[i : i + 2]
    lightpanda, loops = argv[0], [a.rsplit("=", 1) for a in argv[1:]]
    www = tempfile.mkdtemp(prefix="dasher-lightpanda-")
    os.makedirs(os.path.join(www, "micro"))
    with open(os.path.join(www, "micro", "dom.html"), "w") as f:
        f.write(page())
    with open(os.path.join(www, "micro", "dom.js"), "w") as f:
        f.write(open(os.path.join(HERE, "dom.js")).read())

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(Quiet, directory=www)
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}/micro/dom.html"
    env = dict(os.environ, LIGHTPANDA_DISABLE_TELEMETRY="true")

    def cpu(url):
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        out = subprocess.run(
            [lightpanda, "fetch", "--dump", "html", url],
            env=env, capture_output=True, text=True,
        )
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        if out.returncode != 0:
            sys.exit(f"{url}: lightpanda failed: {out.stderr[-500:]}")
        return after.ru_utime - before.ru_utime + after.ru_stime - before.ru_stime

    print("| benchmark | iterations | Lightpanda ns per op |")
    print("| --- | ---: | ---: |")
    for name, n in loops:
        url = f"{base}?{urllib.parse.quote(name, safe='')}&{n}"
        baseline, loaded = [], []
        for _ in range(runs):
            baseline.append(cpu(base))
            loaded.append(cpu(url))
        ns = (statistics.median(loaded) - statistics.median(baseline)) * 1e9 / int(n)
        print(f"| {name} | {n} | {ns:,.0f} |")


if __name__ == "__main__":
    main(sys.argv[1:])
