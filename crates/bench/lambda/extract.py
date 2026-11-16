"""extract.py TAG: one row per run, from the raw outputs under data/TAG/live
(as `aws s3 sync` leaves them), into data/TAG-live.jsonl.

A row is the runner's measurements, what the tool reported about the load,
and what its HTML holds: text length, element counts, bot-wall keywords and
the token set of the visible text, compared with every other run of the
site. BENCH_DATA names another data directory."""
import collections, glob, html, json, os, re, sys, tarfile
DATA = os.environ.get("BENCH_DATA") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
tag, phase = sys.argv[1], "live"
STRIP = re.compile(r"<(script|style|noscript)\b[\s\S]*?</\1\s*>", re.I)
COMMENT = re.compile(r"<!--[\s\S]*?-->")
CJK = re.compile(r"[぀-ヿ㐀-鿿가-힯]")
BLOCK = re.compile(r"just a moment|attention required|access denied|access to this page has been denied|403 forbidden|"
                   r"client challenge|your access has been blocked|you have been blocked|pardon our interruption|"
                   r"are you a robot|robot or human|verify(?:ing)? (?:that )?you are (?:a )?human|human verification|captcha|"
                   r"incapsula|ddos-guard|security check|checking your browser|enable javascript and cookies|"
                   r"request unsuccessful|unusual traffic|bot detection|blocked by|not a robot|datadome|perimeterx|"
                   r"px-captcha|cf-chl|challenge-platform|errors\.edgesuite|request blocked|forbidden", re.I)
def text_of(doc):
    t = STRIP.sub(" ", doc); t = COMMENT.sub(" ", t)
    t = re.sub(r"<[^>]+>", " ", t)
    return re.sub(r"\s+", " ", html.unescape(t)).strip()
def tokens(txt):
    out = set()
    for w in re.findall(r"\w+", txt.lower()):
        if CJK.search(w):
            out.update(w[i:i + 2] for i in range(len(w) - 1))
        elif len(w) >= 3 and not w.isdigit():
            out.add(w)
    return out
def feats(doc):
    txt = text_of(doc)
    m = re.search(r"<title[^>]*>([\s\S]*?)</title>", doc, re.I)
    title = html.unescape(m.group(1)).strip()[:120] if m else ""
    head = (title + " " + txt[:1500])
    return {"html": len(doc), "text": len(txt), "title": title,
            "n_el": len(re.findall(r"<[a-zA-Z]", doc)), "n_a": len(re.findall(r"<a[\s>]", doc, re.I)),
            "n_img": len(re.findall(r"<img[\s>]", doc, re.I)), "n_script": len(re.findall(r"<script[\s>]", doc, re.I)),
            "n_iframe": len(re.findall(r"<iframe[\s>]", doc, re.I)),
            "n_tmpl": len(re.findall(r"<template[\s>]", doc, re.I)),
            "block_kw": (BLOCK.search(head).group(0).lower() if BLOCK.search(head) else None),
            "txt_head": txt[:200]}, tokens(txt)
def metric(mt, name, label=None):
    tot = 0.0; found = False
    for m in re.finditer(r"^" + re.escape(name) + r"(\{[^}]*\})? ([\d.eE+-]+)$", mt, re.M):
        if label and (not m.group(1) or label not in m.group(1)): continue
        tot += float(m.group(2)); found = True
    return tot if found else None
def err_rec(e):
    if isinstance(e, str): return {"kind": "", "message": e[:200], "src": ""}
    st = e.get("stack") or ""
    m = re.search(r"https?://[^/\s:)]+", st)
    return {"kind": e.get("kind") or "", "message": (e.get("message") or "")[:200], "src": m.group(0) if m else ""}
def parse(variant, out, err):
    f = {}; doc = ""
    try:
        o, end = json.JSONDecoder().raw_decode(out)
    except Exception:
        return {"parse": "fail", "out_head": out[:200], "err_tail": err[-600:]}, ""
    if variant.startswith("chromium"):
        doc = o.get("html") or ""
        bt = o.get("by_type") or {}
        f.update(status=o.get("status"), fatal=o.get("error"), capped=o.get("capped"), reqs=o.get("requests"),
                 resp_bytes=o.get("bytes"), js_bytes=o.get("js_bytes"), js_reqs=o.get("js_requests"),
                 load_ms=o.get("load_ms"), elapsed_ms=o.get("elapsed_ms"), launch_ms=o.get("launch_ms"),
                 final_url=o.get("final_url"), failed=o.get("failed"), inflight_end=o.get("inflight_at_end"),
                 core_reqs=sum(bt.get(t, [0, 0])[0] for t in ("Document", "Script", "XHR", "Fetch")),
                 img_reqs=bt.get("Image", [0, 0])[0], img_bytes=bt.get("Image", [0, 0])[1],
                 err_tail=err[-300:] if err.strip() else "")
    elif variant.startswith("dasher"):
        doc = o.get("html") or ""
        errs = o.get("errors") or []
        f.update(status=o.get("status"), timed_out=o.get("timed_out"), settle=o.get("settle"),
                 budget=o.get("budget_exhausted"), reqs=o.get("requests"), elapsed_ms=o.get("elapsed_ms"),
                 n_console=len(o.get("console") or []), n_err=len(errs), fatal=o.get("error"),
                 final_url=o.get("url"),
                 errs=[err_rec(e) for e in errs[:40]],
                 err_tail=err[-600:] if err.strip() else "")
    else:
        doc = o.get("content") or ""
        mt = out[end:]
        f.update(status=o.get("http_status"), fatal=o.get("error"), bot_challenge=o.get("bot_challenge"),
                 page_status=o.get("page_status"), final_url=o.get("url"),
                 reqs=metric(mt, "http_requests_total"), resp_bytes=metric(mt, "http_response_size_bytes_sum"),
                 n_err=metric(mt, "js_errors_total"), n_script_err=metric(mt, "script_errors_total"),
                 heap=metric(mt, "js_heap_size_bytes_sum"), heap_limit=metric(mt, "js_heap_limits_total"),
                 http_err={k: metric(mt, "http_error_total", f'reason="{k}"') for k in ("timeout", "connect", "tls", "too_large", "aborted", "bot_challenge", "other")},
                 http_4xx=metric(mt, "http_status_total", 'category="4xx"'), http_5xx=metric(mt, "http_status_total", 'category="5xx"'),
                 redirects=metric(mt, "http_redirects_total"),
                 log_levels=dict(collections.Counter(re.findall(r"\$level=(\w+)", err))),
                 log_msgs=dict(collections.Counter(re.findall(r'\$msg="([^"]*)"', err)).most_common(12)),
                 err_tail=err[-600:] if err.strip() else "")
    return f, doc
out_path = f"{DATA}/{tag}-{phase}.jsonl"
n = 0
with open(out_path, "w") as w:
    for jf in sorted(glob.glob(f"{DATA}/{tag}/{phase}/*.json")):
        res = json.load(open(jf))
        tf = tarfile.open(jf[:-5] + ".tar.gz")
        files = {m.name.lstrip("./"): m for m in tf.getmembers() if m.isfile()}
        def rd(name):
            return tf.extractfile(files[name]).read().decode("utf8", "replace") if name in files else ""
        toks = {}
        rows = []
        for row in res["rows"]:
            base = f"{row['variant']}-{row['mode']}-{row['run']}"
            f, doc = parse(row["variant"], rd(base + ".out"), rd(base + ".err"))
            ff, tk = feats(doc)
            row.update(f); row.update(ff)
            ch = row["variant"].startswith("chromium")
            # What a run cost: for Chromium, every process it started (the driver left out).
            row["cpu_t"] = row.get("tree_cpu") if ch else row.get("cpu")
            row["mem_kb"] = row.get("tree_pss_kb") if ch else row.get("maxrss_kb")
            row.update(cpu_part=res["sys"].get("CPU part"), slug=res["slug"], url=(res.get("site") or {}).get("url"), stratum=(res.get("site") or {}).get("stratum"))
            toks[(row["variant"], row["mode"], row["run"])] = tk
            rows.append(row)
        # token-set similarity between each pair of runs in the same mode
        for row in rows:
            a = toks[(row["variant"], row["mode"], row["run"])]
            sim = {}
            for (v, m, i), b in toks.items():
                if m != row["mode"] or (v, i) == (row["variant"], row["run"]): continue
                u = len(a | b)
                sim[f"{v}:{i}"] = [round(len(a & b) / u, 3) if u else 1.0, len(a - b), len(b - a)]
            row["n_tok"] = len(a); row["sim"] = sim
            w.write(json.dumps(row) + "\n"); n += 1
print("rows", n, "->", out_path)
