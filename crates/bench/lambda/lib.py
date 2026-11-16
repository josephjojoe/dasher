"""Shared by report.py: how a run is classified, and per-site summaries."""
import collections, json, math, os, statistics
HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.environ.get("BENCH_DATA") or os.path.join(HERE, "data")
T = ("dasher", "lightpanda", "chromium")
BLOCK_STATUS = {401, 403, 405, 418, 429, 430, 503, 202, 400}
def load(name):
    return [json.loads(l) for l in open(f"{DATA}/{name}.jsonl")]
def cls(r):
    """Outcome of one run."""
    if r.get("killed"): return "hung"
    if r["exit"] is not None and r["exit"] < 0: return "crash"
    if r.get("parse") == "fail": return "fail"
    lp = r["variant"].startswith("lightpanda")
    st = r.get("status") or 0
    if lp and (r.get("fatal") == "BotChallenge" or r.get("bot_challenge")): return "blocked"
    if r.get("fatal"): return "fail"
    if r["exit"] != 0: return "fail"
    if st in BLOCK_STATUS and (r.get("block_kw") or r["text"] < 1500): return "blocked"
    if r.get("block_kw") and r["text"] < 1200: return "blocked"
    if st >= 400: return "http_error"
    if r["text"] < 40: return "empty"
    return "ok"
ORDER = ["ok", "empty", "http_error", "blocked", "fail", "hung", "crash"]
def site_cls(rs):
    c = collections.Counter(cls(r) for r in rs)
    top = max(c.values())
    return max((k for k, v in c.items() if v == top), key=ORDER.index)
def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None
def gm(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs))
def pct(xs, p):
    xs = sorted(xs); 
    if not xs: return None
    k = (len(xs) - 1) * p / 100; f = math.floor(k); c = math.ceil(k)
    return xs[f] + (xs[c] - xs[f]) * (k - f)
def group(rows, mode):
    by = collections.defaultdict(lambda: collections.defaultdict(list))
    for r in rows:
        if r["mode"] == mode: by[r["slug"]][r["variant"]].append(r)
    return by
def summarize(by, variants=T):
    """One record per site: medians per tool over runs, outcome, similarity."""
    out = []
    for slug, vs in by.items():
        if not all(vs.get(v) for v in variants): continue
        s = {"slug": slug, "url": vs[variants[0]][0].get("url"), "stratum": vs[variants[0]][0].get("stratum")}
        for v in variants:
            rs = vs[v]
            okr = [r for r in rs if cls(r) == "ok"] or rs
            d = {k: med([r.get(k) for r in okr]) for k in ("cpu_t", "mem_kb", "core_reqs", "js_bytes", "img_reqs", "load_ms", "tree_rss_kb", "cpu", "wall", "user", "sys", "maxrss_kb", "text", "html", "n_el", "n_a", "n_img",
                                                             "n_tok", "reqs", "n_err", "resp_bytes", "heap", "nvcsw", "minflt", "elapsed_ms")}
            d["cls"] = site_cls(rs); d["runs"] = len(rs); d["classes"] = dict(collections.Counter(cls(r) for r in rs))
            d["cpu_runs"] = [r["cpu_t"] for r in rs]; d["capped"] = [r.get("capped") for r in rs if r.get("capped")]; d["status"] = collections.Counter(r.get("status") for r in rs).most_common(1)[0][0]
            d["title"] = rs[0].get("title"); d["budget"] = [r.get("budget") for r in rs if r.get("budget")]
            d["timed_out"] = sum(1 for r in rs if r.get("timed_out"))
            s[v] = d
        # similarity: cross-tool vs within-tool, medians over run pairs
        def sims(a, b):
            xs = []
            for r in vs[a]:
                for k, val in r.get("sim", {}).items():
                    if k.split(":")[0] == b: xs.append(val)
            return xs
        s["sim"] = {}
        for a in variants:
            for b in variants:
                x = sims(a, b)
                s["sim"][f"{a}|{b}"] = med([v[0] for v in x])
        out.append(s)
    return out
def ratio_line(rs, a, b, k="cpu", floor=0.005):
    x = [max(s[a][k], floor) / max(s[b][k], floor) for s in rs]
    ta = sum(s[a][k] for s in rs); tb = sum(s[b][k] for s in rs)
    return (f"total {ta:.1f} vs {tb:.1f} = {ta / tb:.2f}x | per-site median {statistics.median(x):.2f}x, geomean {gm(x):.2f}x, "
            f"p10 {pct(x, 10):.2f} p90 {pct(x, 90):.2f} | {a} lower on {sum(1 for v in x if v < 1 / 1.1)}, within 10% on "
            f"{sum(1 for v in x if 1 / 1.1 <= v <= 1.1)}, higher on {sum(1 for v in x if v > 1.1)}")
