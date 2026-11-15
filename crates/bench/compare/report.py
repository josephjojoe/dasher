#!/usr/bin/env python3
"""Summarize a run.py output directory: dasher against Lightpanda.

usage: report.py OUT [--rec REC] [--sites FILE] [--per-site] [--json PATH]

Per site and browser: median CPU, peak RSS and words over the runs. A run
is usable if it exited 0, was not killed and kept at least 20 words.
Sites are classed as the audit of babeb38 did:
  comparable  both usable, word-set overlap >= 0.5 (shared distinct words
              over the smaller set) and word counts within 1.67x
  thin        both usable, one kept much less text (the other's words
              over 1.67x its own, or overlap < 0.5): counted against it
  fail        one usable and the other not
  neither     neither usable
Matched heavy pages: comparable, neither on a budget or killed, both
fetched the same JavaScript bytes within 10%, dasher or Lightpanda over
1 CPU s.
"""

import argparse
import glob
import json
import os
import statistics as st
import sys
from collections import defaultdict

MIN_WORDS = 20


def load(out, browser):
    runs = defaultdict(list)
    for f in glob.glob(os.path.join(out, browser, "*.json")):
        try:
            m = json.load(open(f))
        except ValueError:
            continue
        t = f[:-5] + ".txt"
        if os.path.exists(t) and m["words"] < 400:
            m["wall_page"] = bot_wall(open(t).read())
        runs[m["site"]].append(m)
    return runs


WALL = ("security verification", "verify you are human", "verifying you are human",
        "are you a robot", "not a robot", "access denied", "captcha", "just a moment",
        "unusual traffic", "you have been blocked", "request blocked", "bot verification",
        "enable javascript and cookies", "pardon our interruption", "press hold")


def bot_wall(text):
    t = " ".join(text.split()[:400])
    return len(text.split()) < 400 and any(w in t for w in WALL)


def usable(m):
    return (m["exit"] == 0 and not m.get("killed") and m["words"] >= MIN_WORDS
            and not m.get("wall_page"))


def med(xs):
    return st.median(xs) if xs else 0.0


def wordset(out, browser, site, runs):
    best = None
    for m in sorted(runs, key=lambda m: m["run"]):
        p = os.path.join(out, browser, f"{site}.{m['run']}.txt")
        if usable(m) and os.path.exists(p):
            best = set(open(p).read().split())
            break
    return best or set()


JS_TYPES = ("javascript", "ecmascript")


def js_bytes(rec, site, urls):
    """Bytes of JavaScript among the recorded responses for these URLs."""
    idx = INDEX.get(site)
    if idx is None:
        idx = {}
        d = os.path.join(rec, site)
        for meta in glob.glob(os.path.join(d, "*.meta")):
            try:
                lines = open(meta, encoding="utf-8", errors="replace").read().split("\n")
            except OSError:
                continue
            ct = next((l.split(": ", 1)[1] for l in lines[5:] if l.lower().startswith("content-type: ")), "")
            if any(t in ct.lower() for t in JS_TYPES):
                size = os.path.getsize(meta[:-5] + ".body")
                idx[f"{lines[0]} {lines[1]}"] = size
                idx.setdefault(f"{lines[0]} {lines[3]}", size)
        INDEX[site] = idx
    return sum(idx.get(u, 0) for u in urls)


INDEX = {}


def hit_urls(out, browser, site, run):
    base = os.path.join(out, browser, f"{site}.{run}")
    urls = []
    if browser == "dasher":
        p = base + ".err"
        if os.path.exists(p):
            for line in open(p, encoding="utf-8", errors="replace"):
                if line.startswith("replay: hit "):
                    urls.append(line[len("replay: hit "):].strip())
    else:
        p = base + ".plog"
        if os.path.exists(p):
            for line in open(p, encoding="utf-8", errors="replace"):
                if line.startswith("hit "):
                    urls.append(line[4:].split(" -> ")[0].strip())
    return urls


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rec", default="rec")
    ap.add_argument("--per-site", action="store_true")
    ap.add_argument("--json")
    ap.add_argument("--a", default="dasher")
    ap.add_argument("--b", default="lp")
    a = ap.parse_args()
    D, L = load(a.out, a.a), load(a.out, a.b)
    sites = sorted(set(D) & set(L))
    rows = []
    for s in sites:
        d, l = D[s], L[s]
        du, lu = [m for m in d if usable(m)], [m for m in l if usable(m)]
        dok, lok = len(du) * 2 > len(d), len(lu) * 2 > len(l)
        r = {
            "site": s,
            "d_cpu": med([m["cpu"] for m in d]), "l_cpu": med([m["cpu"] for m in l]),
            "d_rss": med([m["maxrss_mib"] for m in d]), "l_rss": med([m["maxrss_mib"] for m in l]),
            "d_words": med([m["words"] for m in d]), "l_words": med([m["words"] for m in l]),
            "d_wall": med([m["wall"] for m in d]), "l_wall": med([m["wall"] for m in l]),
            "d_budget": sorted({m.get("budget_exhausted") or "" for m in d} - {""}),
            "d_timeout": any(m.get("timed_out") for m in d),
            "d_killed": any(m.get("killed") for m in d), "l_killed": any(m.get("killed") for m in l),
            "d_exit": sorted({m["exit"] for m in d}), "l_exit": sorted({m["exit"] for m in l}),
            "d_panic": any(m.get("panic") for m in d),
            "d_requests": med([m.get("requests") or 0 for m in d]),
        }
        if dok and lok:
            ws_d, ws_l = wordset(a.out, a.a, s, d), wordset(a.out, a.b, s, l)
            ov = len(ws_d & ws_l) / max(1, min(len(ws_d), len(ws_l)))
            r["overlap"] = ov
            ratio = (r["d_words"] + 1) / (r["l_words"] + 1)
            if ov >= 0.5 and 1 / 1.67 <= ratio <= 1.67:
                r["class"] = "comparable"
            elif ratio < 1:
                r["class"] = "thin-dasher"
            else:
                r["class"] = "thin-lp"
        elif lok:
            r["class"] = "fail-dasher"
        elif dok:
            r["class"] = "fail-lp"
        else:
            r["class"] = "neither"
        if r["class"] == "comparable":
            run = min(m["run"] for m in d)
            jd = js_bytes(a.rec, s, set(hit_urls(a.out, a.a, s, run)))
            jl = js_bytes(a.rec, s, set(hit_urls(a.out, a.b, s, run)))
            r["d_js"], r["l_js"] = jd, jl
            same = max(jd, jl) > 0 and abs(jd - jl) <= 0.1 * max(jd, jl)
            r["matched"] = same and not r["d_budget"] and not r["d_killed"] and not r["l_killed"]
            r["heavy"] = r["matched"] and max(r["d_cpu"], r["l_cpu"]) > 1.0
        rows.append(r)

    by = defaultdict(list)
    for r in rows:
        by[r["class"]].append(r)
    comp = by["comparable"]
    print(f"sites with both browsers run: {len(rows)}")
    for c in ("comparable", "thin-dasher", "thin-lp", "fail-dasher", "fail-lp", "neither"):
        print(f"  {c:12} {len(by[c]):4}  {' '.join(r['site'] for r in by[c]) if c != 'comparable' else ''}")

    def summary(rs, label):
        if not rs:
            return
        dc, lc = sum(r["d_cpu"] for r in rs), sum(r["l_cpu"] for r in rs)
        ratios = [r["d_cpu"] / r["l_cpu"] for r in rs if r["l_cpu"] > 0]
        cheaper_d = sum(1 for r in rs if r["d_cpu"] < r["l_cpu"] / 1.1)
        cheaper_l = sum(1 for r in rs if r["l_cpu"] < r["d_cpu"] / 1.1)
        print(f"{label}: n={len(rs)} dasher {dc:.1f} s, Lightpanda {lc:.1f} s, total ratio {dc / lc:.2f}x, "
              f"median ratio {med(ratios):.2f}x, cheaper D/L {cheaper_d}/{cheaper_l}, "
              f"median peak RSS {med([r['d_rss'] for r in rs]):.0f}/{med([r['l_rss'] for r in rs]):.0f} MiB, "
              f"words {sum(r['d_words'] for r in rs):.0f}/{sum(r['l_words'] for r in rs):.0f}")

    summary(comp, "comparable")
    summary([r for r in comp if r.get("matched")], "matched (same JS, no budget)")
    summary([r for r in comp if r.get("heavy")], "matched heavy (>1 s)")
    summary([r for r in rows if r["class"] != "neither"], "all with a usable page from either")
    budget = [r for r in rows if r["d_budget"]]
    print(f"dasher pages ending on a budget: {len(budget)}: "
          + " ".join(f"{r['site']}({','.join(r['d_budget'])})" for r in budget))
    print(f"dasher timed out: {[r['site'] for r in rows if r['d_timeout']]}; killed: "
          f"{[r['site'] for r in rows if r['d_killed']]}; panics: {[r['site'] for r in rows if r['d_panic']]}")
    print(f"Lightpanda killed: {[r['site'] for r in rows if r['l_killed']]}")
    crashes = [r['site'] for r in rows if any(e < 0 or e > 1 for e in r['d_exit'])]
    print(f"dasher abnormal exits: {crashes}")
    if a.per_site:
        for r in sorted(rows, key=lambda r: -r["d_cpu"]):
            print(f"{r['site']:14} {r['class']:12} cpu {r['d_cpu']:6.2f} {r['l_cpu']:6.2f} "
                  f"rss {r['d_rss']:5.0f} {r['l_rss']:5.0f} words {r['d_words']:6.0f} {r['l_words']:6.0f} "
                  f"ov {r.get('overlap', 0):.2f} js {r.get('d_js', 0) / 1e6:5.2f} {r.get('l_js', 0) / 1e6:5.2f} "
                  f"{'H' if r.get('heavy') else ('m' if r.get('matched') else ' ')} "
                  f"{','.join(r['d_budget'])} exits {r['d_exit']} {r['l_exit']}")
    if a.json:
        json.dump(rows, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    sys.exit(main())
