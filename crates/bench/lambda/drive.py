"""drive.py live TAG [--runs N] [--timeout S] [--only slug,...] [--variants a,b]
drive.py startup TAG [--n SANDBOXES] [--runs N]

Fires one asynchronous Lambda invocation per site (or per sandbox, for
startup) and waits until each has left its result in S3. Needs BENCH_BUCKET;
BENCH_SITES names another site list and BENCH_FUNCTION another function."""
import argparse, json, os, re, subprocess, sys, time
import boto3
from concurrent.futures import ThreadPoolExecutor
HERE = os.path.dirname(os.path.abspath(__file__))
BUCKET = os.environ["BENCH_BUCKET"]
def slug(url):
    s = re.sub(r"^https?://(www\.)?", "", url)
    return re.sub(r"[^a-zA-Z0-9]+", "_", s).strip("_")[:60]
ap = argparse.ArgumentParser()
ap.add_argument("phase"); ap.add_argument("tag")
ap.add_argument("--runs", type=int); ap.add_argument("--timeout", type=int)
ap.add_argument("--only"); ap.add_argument("--fn", default=os.environ.get("BENCH_FUNCTION", "dasher-bench")); ap.add_argument("--n", type=int, default=10)
ap.add_argument("--wait", type=int, default=1000)
ap.add_argument("--variants")
a = ap.parse_args()
lam = boto3.client("lambda"); s3 = boto3.client("s3")
if a.phase == "startup":
    # One invocation per sandbox; the tag carries its index.
    events = [{"phase": "startup", "tag": f"{a.tag}-{i}", "runs": a.runs or 10} for i in range(a.n)]
    keys = [f"{e['tag']}/startup/startup-{e['tag']}.json" for e in events]
else:
    sites = json.load(open(os.environ.get("BENCH_SITES", f"{HERE}/sites.json")))["sites"]
    if a.only:
        want = set(a.only.split(",")); sites = [s for s in sites if slug(s["url"]) in want]
    events = []
    for s in sites:
        e = {"phase": a.phase, "tag": a.tag, "site": s}
        if a.runs: e["runs"] = a.runs
        if a.timeout: e["timeout"] = a.timeout
        if a.variants: e["variants"] = a.variants.split(",")
        events.append(e)
    keys = [f"{a.tag}/{a.phase}/{slug(e['site']['url'])}.json" for e in events]
def have():
    out = set(); tok = None
    prefixes = {k.rsplit("/", 1)[0] + "/" for k in keys}
    for p in prefixes:
        kw = {"Bucket": BUCKET, "Prefix": p}
        while True:
            r = s3.list_objects_v2(**kw)
            out |= {o["Key"] for o in r.get("Contents", [])}
            if not r.get("IsTruncated"): break
            kw["ContinuationToken"] = r["NextContinuationToken"]
    return out
done = have()
todo = [e for e, k in zip(events, keys) if k not in done]
print(f"{len(events)} events, {len(todo)} to invoke", flush=True)
def fire(e):
    r = lam.invoke(FunctionName=a.fn, InvocationType="Event", Payload=json.dumps(e).encode())
    return r["StatusCode"]
with ThreadPoolExecutor(32) as ex:
    codes = list(ex.map(fire, todo))
print("invoked", len(codes), set(codes), flush=True)
t0 = time.time(); last = -1
while time.time() - t0 < a.wait:
    n = len(have() & set(keys))
    if n != last:
        print(f"  {int(time.time() - t0)}s: {n}/{len(keys)} results", flush=True); last = n
    if n >= len(keys): break
    time.sleep(10)
missing = [k for k in keys if k not in have()]
print("missing:", missing[:40])
