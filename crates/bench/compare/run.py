#!/usr/bin/env python3
"""Run dasher and Lightpanda on recorded sites and record CPU, memory and text.

usage: run.py --sites FILE --rec DIR --out DIR --runs N --jobs J
              [--browsers dasher,lp] [--dasher BIN] [--lp BIN] [--record]
              [--dasher-args '...'] [--lp-args '...'] [--only a,b]

Each run of a site writes OUT/<browser>/<site>.<run>.json with user+system
CPU (the process's own rusage, all threads), peak RSS, wall time, exit code,
the visible text's word count, and for dasher the JSON fields the CLI
reports. HTML goes to OUT/<browser>/<site>.<run>.html (last run kept).

dasher replays the recording in-process (DASHER_REPLAY); Lightpanda reads
the same recording through tools/proxy.py. With --record, dasher records
(DASHER_RECORD) and the proxy fetches what is missing from the network.
"""

import argparse
import json
import os
import random
import shlex
import signal
import socket
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from text import visible_text, words  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
PORT_LOCK = threading.Lock()
NEXT_PORT = [20000 + random.randrange(0, 20000)]


def free_port():
    with PORT_LOCK:
        while True:
            NEXT_PORT[0] += 1
            p = NEXT_PORT[0]
            with socket.socket() as s:
                try:
                    s.bind(("127.0.0.1", p))
                    return p
                except OSError:
                    continue


def run_measured(cmd, env, out_path, err_path, limit):
    """CPU and peak RSS of cmd alone, measured by tools/rus (a fork of this
    process would report this process's RSS as the child's peak)."""
    start = time.monotonic()
    res = err_path + ".rus"
    if os.path.exists(res):
        os.remove(res)
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        p = subprocess.Popen([os.path.join(HERE, "rus"), res] + cmd, env=env, stdout=out,
                             stderr=err, start_new_session=True)
        killed = False
        try:
            p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            killed = True
            p.wait()
    wall = time.monotonic() - start
    try:
        user, sys_, rss, code = open(res).read().split()
    except (OSError, ValueError):
        user, sys_, rss, code = 0, 0, 0, -9
    os.remove(res) if os.path.exists(res) else None
    return {
        "cpu": float(user) + float(sys_),
        "user": float(user),
        "sys": float(sys_),
        "maxrss_mib": int(rss) / 1024,
        "wall": wall,
        "exit": int(code),
        "killed": killed,
    }


def start_proxy(store, record, log):
    port = free_port()
    cmd = [sys.executable, "-I", os.path.join(HERE, "proxy.py"), "--port", str(port),
           "--ca", ARGS.ca, "--store", store, "--log", log]
    if record:
        cmd.append("--record")
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    line = p.stdout.readline()
    if not line.startswith(b"ready"):
        raise RuntimeError("proxy did not start")
    return p, port


def one(site, url, browser, run):
    d = os.path.join(ARGS.out, browser)
    os.makedirs(d, exist_ok=True)
    base = os.path.join(d, f"{site}.{run}")
    html, err = base + ".html", base + ".err"
    rec = os.path.join(ARGS.rec, site)
    env = dict(os.environ)
    proxy = None
    if browser == "dasher":
        env["DASHER_REPLAY_LOG"] = "1"
        if ARGS.record:
            env["DASHER_RECORD"] = rec
        else:
            env["DASHER_REPLAY"] = rec
        cmd = [ARGS.dasher, "--json"] + shlex.split(ARGS.dasher_args) + [url]
    else:
        plog = base + ".plog"
        if os.path.exists(plog):
            os.remove(plog)
        proxy, port = start_proxy(rec, ARGS.record, plog)
        env["LIGHTPANDA_DISABLE_TELEMETRY"] = "true"
        cmd = [ARGS.lp, "fetch", "--dump", "html", "--http-proxy", f"http://127.0.0.1:{port}",
               "--ca-cert", os.path.join(ARGS.ca, "ca.pem"), "--log-level", "fatal"] \
            + shlex.split(ARGS.lp_args) + [url]
    try:
        m = run_measured(cmd, env, html, err, ARGS.limit)
    finally:
        if proxy:
            proxy.kill()
            proxy.wait()
    with open(html, "rb") as f:
        raw = f.read().decode("utf-8", "replace")
    m["site"], m["browser"], m["run"] = site, browser, run
    if browser == "dasher":
        try:
            j = json.loads(raw)
            doc = j.get("html") or ""
            for k in ("status", "budget_exhausted", "requests", "timed_out"):
                if k in j:
                    m[k] = j[k]
            m["errors"] = len(j.get("errors") or [])
            with open(html, "w", encoding="utf-8") as f:
                f.write(doc)
        except ValueError:
            doc = ""
        with open(err, "rb") as f:
            e = f.read().decode("utf-8", "replace")
        m["hits"] = e.count("replay: hit")
        m["misses"] = e.count("replay: miss")
        m["panic"] = "panicked at" in e
    else:
        doc = raw
        try:
            with open(base + ".plog", encoding="utf-8") as f:
                pl = f.read()
            m["hits"] = pl.count("\nhit ") + pl.startswith("hit ")
            m["misses"] = pl.count("miss ")
        except OSError:
            pass
    text = visible_text(doc)
    w = words(text)
    m["words"] = len(w)
    m["chars"] = len(text)
    with open(base + ".txt", "w", encoding="utf-8") as f:
        f.write(" ".join(w))
    with open(base + ".json", "w") as f:
        json.dump(m, f)
    return m


def main():
    global ARGS
    p = argparse.ArgumentParser()
    p.add_argument("--sites", required=True)
    p.add_argument("--rec", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--runs", type=int, default=3)
    p.add_argument("--jobs", type=int, default=6)
    p.add_argument("--browsers", default="dasher,lp")
    p.add_argument("--dasher", default="target/profiling/dasher")
    p.add_argument("--lp", default="lightpanda")
    p.add_argument("--ca", default="compare-ca")
    p.add_argument("--dasher-args", default="")
    p.add_argument("--lp-args", default="--wait-until networkidle")
    p.add_argument("--limit", type=float, default=90)
    p.add_argument("--record", action="store_true")
    p.add_argument("--only")
    ARGS = p.parse_args()
    sites = []
    for line in open(ARGS.sites):
        if line.strip():
            name, url = line.split()[:2]
            sites.append((name, url))
    if ARGS.only:
        keep = set(ARGS.only.split(","))
        sites = [s for s in sites if s[0] in keep]
    browsers = ARGS.browsers.split(",")
    tasks = []
    for run in range(1, ARGS.runs + 1):
        order = list(sites)
        random.shuffle(order)
        for name, url in order:
            bs = browsers if (run + hash(name)) % 2 else browsers[::-1]
            tasks.append((name, url, bs, run))

    def work(t):
        name, url, bs, run = t
        for b in bs:
            try:
                m = one(name, url, b, run)
                print(f"{name} {b} {run} cpu={m['cpu']:.2f} rss={m['maxrss_mib']:.0f} "
                      f"words={m['words']} exit={m['exit']}", flush=True)
            except Exception as e:  # noqa: BLE001
                print(f"{name} {b} {run} ERROR {e!r}", flush=True)

    with ThreadPoolExecutor(ARGS.jobs) as ex:
        list(ex.map(work, tasks))
    print("done", flush=True)


if __name__ == "__main__":
    main()
