"""cdp.py [--timeout S] [--idle MS] [--settle-cap S] [--flag F]... URL

Loads URL in a freshly started headless Chromium and prints one JSON object
with the page's HTML, as a scraper using Puppeteer's `networkidle0` would
get it: wait for the load event, then for 500 ms with no request in flight,
then read document.documentElement.outerHTML.

Chromium is driven over --remote-debugging-pipe (NUL-separated JSON on file
descriptors 3 and 4), so nothing but the standard library is needed. A page
that never goes idle is taken as it stands `--settle-cap` seconds after its
load event, or `--timeout` seconds after the navigation started.
"""
import argparse, fcntl, json, os, shutil, signal, subprocess, sys, tempfile, time

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("--timeout", type=float, default=45)
ap.add_argument("--idle", type=float, default=500)
ap.add_argument("--settle-cap", type=float, default=10)
ap.add_argument("--flag", action="append", default=[])
ap.add_argument("--chrome", default=os.environ.get("CHROME", "chromium-headless-shell"))
a = ap.parse_args()

profile = tempfile.mkdtemp(prefix="chr-", dir="/tmp")
cmd_r, cmd_w = os.pipe()
out_r, out_w = os.pipe()
# Move all four ends out of the way, then put the child's two on 3 and 4.
cmd_r, cmd_w, out_r, out_w = (fcntl.fcntl(fd, fcntl.F_DUPFD, 20) for fd in (cmd_r, cmd_w, out_r, out_w))
os.closerange(3, 20)
os.dup2(cmd_r, 3, inheritable=True)
os.dup2(out_w, 4, inheritable=True)
os.set_inheritable(cmd_w, False)
os.set_inheritable(out_r, False)
argv = [a.chrome, "--remote-debugging-pipe", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
        "--no-first-run", "--no-default-browser-check", "--mute-audio", "--hide-scrollbars",
        "--disable-breakpad", "--disable-crash-reporter", "--disable-background-networking",
        "--disable-component-update", "--disable-sync", "--metrics-recording-only",
        "--window-size=1920,1080", f"--user-data-dir={profile}", f"--disk-cache-dir={profile}/cache",
        ] + a.flag + ["about:blank"]
t_start = time.time()
proc = subprocess.Popen(argv, pass_fds=(3, 4), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                        stderr=sys.stderr, env={**os.environ, "HOME": "/tmp", "XDG_CONFIG_HOME": profile,
                                                "XDG_CACHE_HOME": profile})
for fd in (3, 4, cmd_r, out_w):
    os.close(fd)

buf = b""
next_id = 0
pending = []  # messages read while waiting for something else


def send(method, params=None, session=None):
    global next_id
    next_id += 1
    m = {"id": next_id, "method": method, "params": params or {}}
    if session:
        m["sessionId"] = session
    os.write(cmd_w, json.dumps(m).encode() + b"\0")
    return next_id


def read(deadline):
    """The next message, or None at the deadline or when Chromium has gone."""
    global buf
    import select
    while b"\0" not in buf:
        left = deadline - time.time()
        if left <= 0:
            return None
        r, _, _ = select.select([out_r], [], [], left)
        if not r:
            return None
        chunk = os.read(out_r, 1 << 20)
        if not chunk:
            return None
        buf += chunk
    raw, _, buf = buf.partition(b"\0")
    return json.loads(raw)


def call(method, params=None, session=None, deadline=None):
    i = send(method, params, session)
    deadline = deadline or time.time() + 20
    while True:
        m = read(deadline)
        if m is None:
            raise TimeoutError(method)
        if m.get("id") == i:
            if "error" in m:
                raise RuntimeError(f"{method}: {m['error'].get('message')}")
            return m.get("result", {})
        pending.append(m)


res = {"url": a.url, "html": "", "status": None, "error": None, "capped": None, "requests": 0, "bytes": 0,
       "js_bytes": 0, "js_requests": 0, "failed": 0, "load_ms": None, "elapsed_ms": None, "by_type": {}}
try:
    targets = call("Target.getTargets")["targetInfos"]
    page = next(t for t in targets if t["type"] == "page")
    sid = call("Target.attachToTarget", {"targetId": page["targetId"], "flatten": True})["sessionId"]
    res["version"] = call("Browser.getVersion").get("product")
    call("Page.enable", session=sid)
    call("Network.enable", session=sid)
    t_nav = time.time()
    res["launch_ms"] = round((t_nav - t_start) * 1000)
    nav = call("Page.navigate", {"url": a.url}, sid, deadline=t_nav + a.timeout)
    frame = nav.get("frameId")
    if nav.get("errorText"):
        res["error"] = nav["errorText"]
    inflight = {}
    types = {}
    loaded = None
    idle_since = None
    hard = t_nav + a.timeout
    while res["error"] is None:
        now = time.time()
        if loaded and not inflight:
            idle_since = idle_since or now
            if now - idle_since >= a.idle / 1000:
                break
        else:
            idle_since = None
        if now >= hard:
            res["capped"] = "timeout"
            break
        if loaded and now - loaded >= a.settle_cap:
            res["capped"] = "settle"
            break
        wake = min(hard, (idle_since + a.idle / 1000) if idle_since else now + 0.25,
                   (loaded + a.settle_cap) if loaded else hard)
        m = pending.pop(0) if pending else read(max(wake, now + 0.001))
        if m is None:
            if proc.poll() is not None:
                res["error"] = f"chromium exited {proc.returncode}"
            continue
        if m.get("sessionId") != sid:
            continue
        ev, p = m.get("method"), m.get("params", {})
        if ev == "Network.requestWillBeSent":
            if p["requestId"] not in inflight:
                res["requests"] += 1
            inflight[p["requestId"]] = now
            types[p["requestId"]] = p.get("type")
        elif ev == "Network.responseReceived":
            if p.get("type") == "Document" and p.get("frameId") == frame:
                res["status"] = p["response"].get("status")
                res["final_url"] = p["response"].get("url")
        elif ev == "Network.loadingFinished":
            inflight.pop(p["requestId"], None)
            n = p.get("encodedDataLength", 0)
            t = types.get(p["requestId"]) or "Other"
            res["bytes"] += n
            bt = res["by_type"].setdefault(t, [0, 0])
            bt[0] += 1
            bt[1] += n
            if t == "Script":
                res["js_bytes"] += n
                res["js_requests"] += 1
        elif ev == "Network.loadingFailed":
            inflight.pop(p["requestId"], None)
            res["failed"] += 1
        elif ev == "Page.loadEventFired":
            loaded = now
            res["load_ms"] = round((now - t_nav) * 1000)
    res["inflight_at_end"] = len(inflight)
    if res["error"] is None or res["status"]:
        r = call("Runtime.evaluate", {"expression": "document.documentElement.outerHTML", "returnByValue": True},
                 sid, deadline=time.time() + 20)
        res["html"] = r.get("result", {}).get("value") or ""
        r = call("Runtime.evaluate", {"expression": "document.title", "returnByValue": True}, sid)
        res["title"] = r.get("result", {}).get("value")
    res["elapsed_ms"] = round((time.time() - t_nav) * 1000)
except Exception as e:  # noqa: BLE001
    res["error"] = res["error"] or f"{type(e).__name__}: {e}"
finally:
    try:
        send("Browser.close")
        proc.wait(timeout=5)
    except Exception:  # noqa: BLE001
        proc.kill()  # the runner kills whatever else is left in the session
    shutil.rmtree(profile, ignore_errors=True)
json.dump(res, sys.stdout)
sys.exit(0 if res["html"] else 1)
