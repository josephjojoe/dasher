"""startup.py TAG: process start to exit on a page that costs nothing, from
the results of `drive.py startup TAG` under data/TAG-*/startup."""
import glob, json, os, statistics as st, sys
DATA = os.environ.get("BENCH_DATA") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
tag = sys.argv[1]
rows, sandboxes = [], 0
for f in sorted(glob.glob(f"{DATA}/{tag}-*/**/*.json", recursive=True)):
    d = json.load(open(f))
    if d.get("phase") != "startup":
        continue
    sandboxes += 1
    rows += d["rows"]
def q(xs, p):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(p * len(xs)))]
print(f"sandboxes {sandboxes}, runs {len(rows)}")
print(f"{'mode':10} {'tool':11} {'n':>4} {'wall ms med':>11} {'p90':>7} {'CPU ms med':>10} {'peak MiB med':>12} {'exit!=0':>7}")
for mode in ("version", "tiny-load", "tiny"):
    for v in ("dasher", "lightpanda", "chromium"):
        rs = [r for r in rows if r["mode"] == mode and r["variant"] == v and r.get("wall") is not None]
        tree = v == "chromium" and mode != "version"
        cpu = [(r["tree_cpu"] if tree else r["cpu"]) * 1000 for r in rs]
        mem = [(r["tree_pss_kb"] if tree else r["maxrss_kb"]) / 1024 for r in rs]
        wall = [r["wall"] * 1000 for r in rs]
        bad = sum(1 for r in rs if r["exit"] != 0)
        print(f"{mode:10} {v:11} {len(rs):4} {st.median(wall):11.0f} {q(wall, .9):7.0f} {st.median(cpu):10.0f} {st.median(mem):12.1f} {bad:7}")
