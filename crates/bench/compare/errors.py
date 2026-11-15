#!/usr/bin/env python3
"""Run dasher once per recorded site and tally page errors by message.

usage: BIN=path/to/dasher errors.py SITES REC OUT.json [jobs]

BIN is a build with the `replay` feature (default target/profiling/dasher).
"""
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor

sites = [l.split()[:2] for l in open(sys.argv[1]) if l.strip()]
rec, out = sys.argv[2], sys.argv[3]
jobs = int(sys.argv[4]) if len(sys.argv) > 4 else 6
BIN = os.environ.get("BIN", "target/profiling/dasher")


def one(s):
    name, url = s
    p = subprocess.run([BIN, "--json", url], env={"DASHER_REPLAY": f"{rec}/{name}"},
                       capture_output=True, timeout=120)
    try:
        j = json.loads(p.stdout)
    except ValueError:
        return name, [f"EXIT {p.returncode}: {p.stderr[-300:]!r}"]
    msgs = [e.get("message", "") for e in j.get("errors", [])]
    msgs += [c["text"].split("\n")[0] for c in j.get("console", []) if c.get("level") == "error"]
    return name, msgs


with ThreadPoolExecutor(jobs) as ex:
    res = dict(ex.map(one, sites))
json.dump(res, open(out, "w"), indent=1)
by = defaultdict(set)
for name, msgs in res.items():
    for m in msgs:
        key = re.sub(r"\d+", "N", m)[:140]
        by[key].add(name)
for key, names in sorted(by.items(), key=lambda kv: -len(kv[1]))[:70]:
    print(f"{len(names):3} {key}  [{' '.join(sorted(names)[:6])}]")
