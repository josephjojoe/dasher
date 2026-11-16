"""drive.py live TAG [--runs N] [--timeout S] [--only slug,...] [--variants a,b]
drive.py startup TAG [--n SANDBOXES] [--runs N]

Fires one asynchronous Lambda invocation per site (or per sandbox, for
startup) and waits until each has left its result in S3. An invocation that
fails leaves its error there instead; it is printed and the invocation made
again, --retries times. Exits with status 1 if a result is missing at the
end. Needs BENCH_BUCKET; BENCH_SITES names another site list and
BENCH_FUNCTION another function."""
import argparse, json, os, re, subprocess, sys, time
import boto3
from botocore.config import Config
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
ap.add_argument("--retries", type=int, default=1)
ap.add_argument("--variants")
a = ap.parse_args()
# A call that stalls is given up and made again rather than waited on.
cfg = Config(connect_timeout=10, read_timeout=30, retries={"max_attempts": 5, "mode": "standard"})
lam = boto3.client("lambda", config=cfg); s3 = boto3.client("s3", config=cfg)
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
def error_key(k):
    return k[:-len(".json")] + ".error.txt"
def fire(e):
    r = lam.invoke(FunctionName=a.fn, InvocationType="Event", Payload=json.dumps(e).encode())
    return r["StatusCode"]
def invoke(events):
    with ThreadPoolExecutor(32) as ex:
        codes = list(ex.map(fire, events))
    print("invoked", len(codes), set(codes), flush=True)
event_of = dict(zip(keys, events))
found = have()
# The errors of an earlier run with this tag are not this run's.
for k in keys:
    if k not in found and error_key(k) in found:
        s3.delete_object(Bucket=BUCKET, Key=error_key(k))
pending = {k for k in keys if k not in found}
print(f"{len(events)} events, {len(pending)} to invoke", flush=True)
invoke([event_of[k] for k in keys if k in pending])
tries = dict.fromkeys(pending, 0); failed = []
t0 = time.time(); last = -1
while pending and time.time() - t0 < a.wait:
    time.sleep(10 if last >= 0 else 2)
    found = have()
    pending -= found
    again = []
    for k in sorted(pending):
        if error_key(k) not in found: continue
        err = s3.get_object(Bucket=BUCKET, Key=error_key(k))["Body"].read().decode(errors="replace")
        print(f"  failed: {k}: {err.strip().splitlines()[-1]}", flush=True)
        if tries[k] < a.retries:
            tries[k] += 1
            s3.delete_object(Bucket=BUCKET, Key=error_key(k))
            again.append(event_of[k])
        else:
            pending.discard(k); failed.append(k)
    if again: invoke(again)
    n = len(keys) - len(pending) - len(failed)
    if n != last:
        print(f"  {int(time.time() - t0)}s: {n}/{len(keys)} results", flush=True); last = n
# What is still pending left neither a result nor an error: it timed out or
# its sandbox was killed, and the function's log says which.
if failed: print("failed:", failed)
if pending: print("no result and no error:", sorted(pending))
sys.exit(1 if failed or pending else 0)
