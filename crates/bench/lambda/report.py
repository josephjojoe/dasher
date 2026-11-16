"""report.py TAG: dasher against Lightpanda and Chromium, five variants per sandbox.

Reads data/TAG-live.jsonl (from extract.py), prints the report and writes
data/TAG-sites.csv, one row per site. Only sites in sites.json (or
BENCH_SITES) are reported."""
import collections, csv, json, os, re, statistics, sys
from lib import *

tag = sys.argv[1] if len(sys.argv) > 1 else "r1"
D, L, C, L30, CN = "dasher", "lightpanda", "chromium", "lightpanda-30s", "chromium-noimg"
V = (D, L, C, L30, CN)
H = (D, L, C)
rows = load(f"{tag}-live")


def slug(url):
    s = re.sub(r"^https?://(www\.)?", "", url)
    return re.sub(r"[^a-zA-Z0-9]+", "_", s).strip("_")[:60]


meta = {slug(s["url"]): s for s in json.load(open(os.environ.get("BENCH_SITES") or f"{HERE}/sites.json"))["sites"]}
measured = len({r["slug"] for r in rows})
rows = [r for r in rows if r["slug"] in meta]
S = summarize(group(rows, "live"), V)
for s in S:
    s["set"] = meta[s["slug"]]["list"]
    s["stratum"] = meta[s["slug"]]["stratum"]
print(f"sites measured: {measured}; in the site list: {len(meta)}; with all five variants: {len(S)}; runs: {len(rows)}")
print("runs per variant:", dict(collections.Counter(r["variant"] for r in rows)))
print("sandbox CPUs (by /proc/cpuinfo part):", dict(collections.Counter(r.get("cpu_part") for r in rows if r["variant"] == D and r["run"] == 0)))
print("sets:", dict(collections.Counter(s["set"] for s in S)), "strata:", dict(collections.Counter(s["stratum"] for s in S)))

print("\n== OUTCOMES (per site, majority of 3 runs)")
for v in V:
    c = collections.Counter(s[v]["cls"] for s in S)
    print(f"  {v:15s}", {k: c[k] for k in ORDER if c[k]})
print("  signals/kills by variant:", {v: sum(1 for r in rows if r["variant"] == v and ((r["exit"] or 0) < 0 or r.get("killed"))) for v in V})
ok = lambda s, v: s[v]["cls"] == "ok"
print("\n== OVERLAP of ok sites")
for a, b in ((D, L), (D, C), (L, C)):
    both = sum(ok(s, a) and ok(s, b) for s in S)
    print(f"  {a} & {b}: both ok {both}; only {a} {sum(ok(s, a) and not ok(s, b) for s in S)}; only {b} {sum(ok(s, b) and not ok(s, a) for s in S)}; neither {sum(not ok(s, a) and not ok(s, b) for s in S)}")
print(f"  all three ok: {sum(all(ok(s, v) for v in H) for s in S)}; none ok: {sum(not any(ok(s, v) for v in H) for s in S)}")
print("  outcome of the other two where one is not ok:")
for v in H:
    others = [o for o in H if o != v]
    c = collections.Counter((s[v]["cls"], tuple(s[o]["cls"] == "ok" for o in others)) for s in S if not ok(s, v))
    print(f"    {v} not ok ({others[0]} ok, {others[1]} ok):", {f"{k[0]}:{'+'.join('Y' if x else 'n' for x in k[1])}": n for k, n in sorted(c.items())})

print("\n== ONE-SIDED: dasher not ok where Chromium is ok")
for s in S:
    if not ok(s, D) and ok(s, C):
        print(f"  {s['slug'][:40]:40s} dasher {s[D]['cls']:10s} ({s[D]['status']}, text {s[D]['text']:.0f}) | lightpanda {s[L]['cls']:10s} | chromium text {s[C]['text']:.0f}")
print("\n== ONE-SIDED: dasher ok where Chromium is not")
for s in S:
    if ok(s, D) and not ok(s, C):
        print(f"  {s['slug'][:40]:40s} chromium {s[C]['cls']:10s} ({s[C]['status']}, text {s[C]['text']:.0f}) | lightpanda {s[L]['cls']:10s} | dasher text {s[D]['text']:.0f}")
print("\n== ONE-SIDED: dasher vs lightpanda")
for s in S:
    if ok(s, D) != ok(s, L):
        print(f"  {s['slug'][:40]:40s} dasher {s[D]['cls']:10s} ({s[D]['status']}, text {s[D]['text']:.0f}) | lightpanda {s[L]['cls']:10s} ({s[L]['status']}, text {s[L]['text']:.0f}) | chromium {s[C]['cls']}")


def sim(s, a, b):
    return s["sim"].get(f"{a}|{b}")


print("\n== CONTENT against Chromium (sites where both are ok; token-set Jaccard similarity of visible text)")
for v in (D, L, L30, CN):
    ss = [s for s in S if ok(s, v) and ok(s, C) and sim(s, v, C) is not None]
    x = [sim(s, v, C) for s in ss]
    selfc = [sim(s, C, C) for s in ss if sim(s, C, C) is not None]
    tr = [(s[v]["text"] + 1) / (s[C]["text"] + 1) for s in ss]
    print(f"  {v:15s} n={len(ss)} similarity median {statistics.median(x):.3f} p25 {pct(x, 25):.3f} p10 {pct(x, 10):.3f} | >=0.9: {sum(q >= .9 for q in x)} >=0.7: {sum(q >= .7 for q in x)} <0.5: {sum(q < .5 for q in x)}"
          f" | text length ratio median {statistics.median(tr):.2f}; under half Chromium's text: {sum(q < .5 for q in tr)}, over double: {sum(q > 2 for q in tr)}")
    if v == D:
        print(f"  (Chromium against itself across runs: median {statistics.median(selfc):.3f}, >=0.9 on {sum(q >= .9 for q in selfc)} of {len(selfc)})")
print("  dasher vs lightpanda where both ok:", end=" ")
ss = [s for s in S if ok(s, D) and ok(s, L) and sim(s, D, L) is not None]
x = [sim(s, D, L) for s in ss]
print(f"n={len(ss)} median {statistics.median(x):.3f} >=0.9: {sum(q >= .9 for q in x)} <0.5: {sum(q < .5 for q in x)}")
print("  dasher much less text than Chromium (both ok, dasher under half):")
for s in sorted((s for s in S if ok(s, D) and ok(s, C) and (s[D]["text"] + 1) / (s[C]["text"] + 1) < .5), key=lambda s: s[D]["text"] / (s[C]["text"] + 1)):
    print(f"    {s['slug'][:40]:40s} dasher {s[D]['text']:.0f} lightpanda {s[L]['text']:.0f} ({s[L]['cls']}) chromium {s[C]['text']:.0f} | dasher errors {s[D]['n_err']} budget {s[D]['budget'][:1]}")
print("  lightpanda much less text than Chromium (both ok, under half):")
for s in sorted((s for s in S if ok(s, L) and ok(s, C) and (s[L]["text"] + 1) / (s[C]["text"] + 1) < .5), key=lambda s: s[L]["text"] / (s[C]["text"] + 1)):
    print(f"    {s['slug'][:40]:40s} lightpanda {s[L]['text']:.0f} dasher {s[D]['text']:.0f} ({s[D]['cls']}) chromium {s[C]['text']:.0f} | lightpanda wall {s[L]['wall']:.1f}")


def table(rs, vs, label):
    print(f"\n-- {label}: n={len(rs)}")
    if len(rs) < 3:
        return
    for k, name, f in (("cpu_t", "CPU s", 1), ("wall", "wall s", 1), ("mem_kb", "peak mem MiB", 1 / 1024)):
        for v in vs:
            a = [s[v][k] * f for s in rs]
            print(f"  {name:13s} {v:15s} median {statistics.median(a):7.2f} mean {statistics.mean(a):7.2f} p90 {pct(a, 90):7.2f} p99 {pct(a, 99):7.2f} max {max(a):7.1f} sum {sum(a):8.1f}")
    for a, b in ((D, L), (D, C), (L, C)):
        if a in vs and b in vs:
            print(f"  CPU  {a}/{b}:", ratio_line(rs, a, b, "cpu_t"))
            print(f"  wall {a}/{b}:", ratio_line(rs, a, b, "wall", 0.01))
            print(f"  mem  {a}/{b}:", ratio_line(rs, a, b, "mem_kb", 1))


all3 = [s for s in S if all(ok(s, v) for v in H)]
table(all3, H, "ALL THREE OK")
for st in ("crux", "known"):
    table([s for s in all3 if s["set"] in (st, "both")], H, f"all three ok, {st} list")
same = [s for s in all3 if (sim(s, D, C) or 0) >= .9 and (sim(s, L, C) or 0) >= .9]
table(same, H, "SAME CONTENT: all three ok, dasher and lightpanda each >= 0.9 similar to Chromium")
heavy = [s for s in same if s[C]["cpu_t"] > 2]
table(heavy, H, "same content, heavy pages (Chromium > 2 CPU s)")
light = [s for s in same if s[C]["cpu_t"] <= 2]
table(light, H, "same content, light pages (Chromium <= 2 CPU s)")
dl = [s for s in S if ok(s, D) and ok(s, L)]
table(dl, (D, L), "DASHER AND LIGHTPANDA OK")
m = [s for s in dl if 1 / 1.25 <= (s[D]["reqs"] + 3) / (s[L]["reqs"] + 3) <= 1.25 and (sim(s, D, L) or 0) >= .9]
table(m, (D, L), "WORK-MATCHED dasher vs lightpanda (requests within 1.25x, text similarity >= 0.9)")
nb = lambda s: not s[D]["budget"] and not s[D]["timed_out"]
table([s for s in m if max(s[D]["cpu_t"], s[L]["cpu_t"]) > 1 and nb(s)], (D, L), "work-matched heavy (either > 1 CPU s, no dasher budget)")
dc = [s for s in S if ok(s, D) and ok(s, C)]
table(dc, (D, C), "DASHER AND CHROMIUM OK")
table([s for s in dc if (sim(s, D, C) or 0) >= .9], (D, C), "dasher and Chromium ok, same content (>= 0.9)")

print("\n== SENSITIVITY: Lightpanda's wait cap (default 5 s) against 30 s")
ss = [s for s in S if ok(s, L) and ok(s, L30)]
table(ss, (L, L30), "lightpanda and lightpanda-30s ok")
print("  CPU 30s/default:", ratio_line(ss, L30, L, "cpu_t"))
print("  text 30s/default: total", f"{sum(s[L30]['text'] for s in ss) / sum(s[L]['text'] for s in ss):.3f}", "| sites with >10% more text at 30 s:", sum(s[L30]["text"] > 1.1 * s[L]["text"] + 50 for s in ss))
print("  outcomes changed:", dict(collections.Counter((s[L]["cls"], s[L30]["cls"]) for s in S if s[L]["cls"] != s[L30]["cls"])))
ss = [s for s in S if ok(s, L30) and ok(s, C)]
x = [sim(s, L30, C) for s in ss if sim(s, L30, C) is not None]
print(f"  lightpanda-30s similarity to Chromium: n={len(x)} median {statistics.median(x):.3f} >=0.9 {sum(q >= .9 for q in x)}")
print("\n== SENSITIVITY: Chromium without images")
ss = [s for s in S if ok(s, C) and ok(s, CN)]
table(ss, (C, CN), "chromium and chromium-noimg ok")
print("  CPU noimg/default:", ratio_line(ss, CN, C, "cpu_t"))
print("  wall noimg/default:", ratio_line(ss, CN, C, "wall", 0.01))
print("  mem noimg/default:", ratio_line(ss, CN, C, "mem_kb", 1))
ss3 = [s for s in S if all(ok(s, v) for v in (D, L, C, CN))]
print("  dasher / chromium-noimg, all ok:", ratio_line(ss3, D, CN, "cpu_t"))

print("\n== LIMITS HIT (runs)")
dr = [r for r in rows if r["variant"] == D]
print(f"  dasher: budget_exhausted {dict(collections.Counter(r.get('budget') for r in dr))} timed_out {sum(1 for r in dr if r.get('timed_out'))} runs >= 25 s wall {sum(1 for r in dr if (r['wall'] or 0) >= 25)}")
print(f"     sites with a budget on most runs: {sorted(s['slug'] for s in S if len(s[D]['budget']) >= 2)}")
for v in (L, L30):
    lr = [r for r in rows if r["variant"] == v]
    print(f"  {v}: runs >= 4.9 s wall {sum(1 for r in lr if (r['wall'] or 0) >= 4.9)} (sites, most runs: {sum(1 for s in S if s[v]['wall'] >= 4.9)}), >= 25 s {sum(1 for r in lr if (r['wall'] or 0) >= 25)}, killed {sum(1 for r in lr if r.get('killed'))}")
for v in (C, CN):
    cr = [r for r in rows if r["variant"] == v]
    print(f"  {v}: capped {dict(collections.Counter(r.get('capped') for r in cr))}; sites capped on most runs: {sum(1 for s in S if len(s[v]['capped']) >= 2)}; killed {sum(1 for r in cr if r.get('killed'))}")

print("\n== MEMORY, every run that exited 0 (dasher, lightpanda: peak RSS; chromium: peak summed PSS of its processes)")
for v in V:
    x = [r["mem_kb"] / 1024 for r in rows if r["variant"] == v and r["exit"] == 0 and r.get("mem_kb")]
    print(f"  {v:15s} n={len(x)} p10 {pct(x, 10):.0f} median {statistics.median(x):.0f} p90 {pct(x, 90):.0f} p99 {pct(x, 99):.0f} max {max(x):.0f} | over 256 MiB {sum(q > 256 for q in x)}, over 512 {sum(q > 512 for q in x)}, over 1024 {sum(q > 1024 for q in x)}")
x = [r["tree_rss_kb"] / r["tree_pss_kb"] for r in rows if r["variant"] == C and r["exit"] == 0 and r.get("tree_pss_kb")]
print(f"  chromium summed RSS / summed PSS: median {statistics.median(x):.2f}; processes at once: median {statistics.median(r['nproc'] for r in rows if r['variant'] == C and r['exit'] == 0)}")

print("\n== ORDER EFFECT: median CPU by position in the round (all three ok)")
oks = {s["slug"] for s in all3}
for v in V:
    print(f"  {v:15s}", {pos: round(statistics.median(r["cpu_t"] for r in rows if r["variant"] == v and r["pos"] == pos and r["slug"] in oks), 3) for pos in range(5)},
          "n", {pos: sum(1 for r in rows if r["variant"] == v and r["pos"] == pos) for pos in range(5)})
print("\n== RUN-TO-RUN: per-site spread of CPU over the 3 runs, (max-min)/median, all three ok")
for v in H:
    x = [(max(s[v]["cpu_runs"]) - min(s[v]["cpu_runs"])) / max(statistics.median(s[v]["cpu_runs"]), .01) for s in all3]
    print(f"  {v:15s} median {statistics.median(x):.2f} p90 {pct(x, 90):.2f}")

print("\n== COST per million pages on Lambda arm64 at 3008 MB (mean wall x memory + request fee), all three ok")
for v in V:
    ss = [s for s in S if all(ok(s, q) for q in H) and s[v]["wall"] is not None]
    w = statistics.mean(s[v]["wall"] for s in ss)
    print(f"  {v:15s} mean wall {w:.2f} s  mean CPU {statistics.mean(s[v]['cpu_t'] for s in ss):.2f} s -> ${(w * 3008 / 1024 * 0.0000133334 + 0.2e-6) * 1e6:.0f}")

print("\n== BIGGEST CPU GAPS among same-content sites")
for s in sorted(same, key=lambda s: s[C]["cpu_t"] - s[D]["cpu_t"])[:10]:
    print(f"  dasher dearer than chromium  {s['slug'][:36]:36s} dasher {s[D]['cpu_t']:6.2f} lightpanda {s[L]['cpu_t']:6.2f} chromium {s[C]['cpu_t']:6.2f} reqs {s[D]['reqs']:.0f}/{s[L]['reqs']:.0f}/{s[C]['core_reqs']:.0f} budget {s[D]['budget'][:1]}")
for s in sorted(same, key=lambda s: s[L]["cpu_t"] - s[D]["cpu_t"])[:10]:
    print(f"  dasher dearer than lightpanda {s['slug'][:36]:36s} dasher {s[D]['cpu_t']:6.2f} lightpanda {s[L]['cpu_t']:6.2f} chromium {s[C]['cpu_t']:6.2f} reqs {s[D]['reqs']:.0f}/{s[L]['reqs']:.0f}/{s[C]['core_reqs']:.0f} budget {s[D]['budget'][:1]}")
for s in sorted(same, key=lambda s: s[D]["cpu_t"] - s[L]["cpu_t"])[:10]:
    print(f"  dasher cheaper than lightpanda {s['slug'][:36]:36s} dasher {s[D]['cpu_t']:6.2f} lightpanda {s[L]['cpu_t']:6.2f} chromium {s[C]['cpu_t']:6.2f} reqs {s[D]['reqs']:.0f}/{s[L]['reqs']:.0f}/{s[C]['core_reqs']:.0f}")

with open(f"{DATA}/{tag}-sites.csv", "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["url", "list", "stratum"] + [f"{v}_{k}" for v in V for k in ("outcome", "status", "cpu_s", "wall_s", "peak_mem_mib", "text_chars", "requests")]
               + ["sim_dasher_chromium", "sim_lightpanda_chromium", "sim_dasher_lightpanda", "dasher_budget"])
    for s in sorted(S, key=lambda s: s["url"]):
        row = [s["url"], s["set"], s["stratum"]]
        for v in V:
            d = s[v]
            f2 = lambda x, n=2: "" if x is None else round(x, n)
            row += [d["cls"], d["status"] or "", f2(d["cpu_t"]), f2(d["wall"]), f2((d["mem_kb"] or 0) / 1024, 1), f2(d["text"], 0), f2(d["reqs"], 0)]
        row += [sim(s, D, C) if sim(s, D, C) is not None else "", sim(s, L, C) if sim(s, L, C) is not None else "",
                sim(s, D, L) if sim(s, D, L) is not None else "", (s[D]["budget"] or [""])[0]]
        w.writerow(row)
# To stderr: the report itself is redirected to a file that gets shared.
print(f"wrote {DATA}/{tag}-sites.csv", file=sys.stderr)
