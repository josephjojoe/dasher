"""Lambda bench harness: runs dasher, Lightpanda and Chromium's headless shell
on one site, back to back in the same sandbox, and uploads the raw outputs
and measurements to S3."""
import hashlib, io, json, os, re, socket, subprocess, tarfile, threading, time, traceback
from urllib.parse import urlsplit

ROOT = os.environ.get("LAMBDA_TASK_ROOT", os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "bin")
DASHER, LP, RUNNER = (os.path.join(BIN, n) for n in ("dasher", "lightpanda", "runner"))
CDP = os.path.join(ROOT, "cdp.py")
CHROME = os.environ.get("CHROME", "/usr/bin/chromium-headless-shell")
CHROME_REAL = os.path.realpath(CHROME)
BUCKET = os.environ.get("BENCH_BUCKET", "")
WORK = "/tmp/work"
ENV = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": "/tmp", "LIGHTPANDA_DISABLE_TELEMETRY": "true",
       "TMPDIR": "/tmp", "CHROME": CHROME}

# Warm the page cache so no browser pays a first read of itself from disk
# (a Lambda container image is fetched lazily, block by block).
def _warm():
    paths = [DASHER, LP, RUNNER, CHROME_REAL]
    d = os.path.dirname(CHROME_REAL)
    paths += [os.path.join(d, f) for f in os.listdir(d)] if "chromium" in d else []
    for p in paths:
        try:
            with open(p, "rb") as f:
                while f.read(1 << 20):
                    pass
        except OSError:
            pass
    # One throwaway start of each, which also pulls in their shared libraries.
    for v in ("dasher", "lightpanda", "chromium"):
        cmd, driver = command(v, "data:text/html,<p>warm</p>" if v == "chromium" else "http://127.0.0.1:9/", 10)
        run(cmd, "/tmp/warm", 30, driver)
_WARM = False

# Lambda's sandbox forbids the namespaces Chromium's zygote sets up, even
# with --no-sandbox, so child processes are started directly.
CHROME_FLAGS = ["--no-zygote"]

VARIANTS = ["dasher", "lightpanda", "chromium", "dasher-30s", "lightpanda-30s", "chromium-noimg"]


def slug(url):
    s = re.sub(r"^https?://(www\.)?", "", url)
    return re.sub(r"[^a-zA-Z0-9]+", "_", s).strip("_")[:60]


def versions():
    def v(cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=20, env=ENV).stdout.strip()[:120]
        except Exception as e:  # noqa: BLE001
            return f"error: {e}"
    return {"dasher": v([DASHER, "--version"]), "lightpanda": v([LP, "version"]),
            "chromium": v([CHROME, "--version"])}


def sysinfo():
    info = {"ncpu": os.cpu_count()}
    try:
        ci = open("/proc/cpuinfo").read()
        for key in ("model name", "CPU part", "CPU implementer", "BogoMIPS"):
            m = re.search(key + r"\s*:\s*(.+)", ci)
            if m:
                info[key] = m.group(1)
        info["memtotal_kb"] = int(re.search(r"MemTotal:\s+(\d+)", open("/proc/meminfo").read()).group(1))
    except Exception as e:  # noqa: BLE001
        info["err"] = str(e)
    info["lambda_mem"] = os.environ.get("AWS_LAMBDA_FUNCTION_MEMORY_SIZE")
    info["region"] = os.environ.get("AWS_REGION")
    return info


def run(cmd, base, hard_timeout, skip_root=False):
    """Runs cmd to completion under the static launcher; returns its timing and usage."""
    out, err = base + ".out", base + ".err"
    p = subprocess.run([RUNNER] + (["-x"] if skip_root else []) + [out, err, str(int(hard_timeout))] + cmd,
                       env=ENV, cwd="/tmp", stdin=subprocess.DEVNULL, capture_output=True, text=True)
    try:
        r = json.loads(p.stdout)
    except Exception:  # noqa: BLE001
        r = {"t": time.time(), "wall": None, "cpu": None, "user": None, "sys": None, "maxrss_kb": None,
             "tree_cpu": None, "tree_pss_kb": None, "tree_rss_kb": None,
             "exit": -999, "killed": False, "runner_err": (p.stdout + p.stderr)[-300:]}
    r["out_bytes"] = os.path.getsize(out) if os.path.exists(out) else 0
    r["err_bytes"] = os.path.getsize(err) if os.path.exists(err) else 0
    return r


def command(variant, url, timeout, wait="idle"):
    """The command line of one variant, and whether its direct child is a driver to leave out."""
    if variant.startswith("dasher"):
        cmd = [DASHER, "--json", "-t", str(timeout)]
        if variant == "dasher-30s":
            cmd += ["--wait-budget", str(timeout)]
        if wait == "load":
            cmd += ["--wait", "load"]
        return cmd + [url], False
    if variant.startswith("lightpanda"):
        cmd = [LP, "fetch", "--dump", "html", "--json", "--metrics",
               "--wait-until", "load" if wait == "load" else "networkidle"]
        if variant == "lightpanda-30s":
            cmd += ["--wait-ms", str(timeout * 1000)]
        return cmd + [url], False
    cmd = ["python3", CDP, "--timeout", str(timeout), "--settle-cap", "5"] + [f"--flag={f}" for f in CHROME_FLAGS]
    if wait == "load":
        cmd += ["--idle", "0"]
    if variant == "chromium-noimg":
        cmd += ["--flag=--blink-settings=imagesEnabled=false"]
    return cmd + [url], True


def upload(key, data, ctype="application/json"):
    if not BUCKET:
        os.makedirs("/tmp/out", exist_ok=True)
        with open(os.path.join("/tmp/out", key.replace("/", "__")), "wb") as f:
            f.write(data)
        return
    import boto3
    boto3.client("s3").put_object(Bucket=BUCKET, Key=key, Body=data, ContentType=ctype)


def pack(d):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz", compresslevel=6) as tf:
        tf.add(d, arcname=".")
    return buf.getvalue()


def fresh(d):
    subprocess.run(["rm", "-rf", d])
    os.makedirs(d, exist_ok=True)


def live(event, ctx):
    site = event["site"]
    url = site["url"]
    s = slug(url)
    runs = event.get("runs", 3)
    timeout = event.get("timeout", 30)
    d = os.path.join(WORK, s)
    fresh(d)
    try:
        socket.getaddrinfo(urlsplit(url).hostname, 443)
    except Exception:  # noqa: BLE001
        pass
    rows = []
    vs = event.get("variants", VARIANTS)
    # Each site starts its rotation somewhere else, so over the sites every
    # variant sits in every position equally often.
    off = int(hashlib.sha1(s.encode()).hexdigest(), 16)
    for i in range(runs):
        k = (off + i * 2) % len(vs)
        order = vs[k:] + vs[:k]
        for pos, v in enumerate(order):
            if ctx.get_remaining_time_in_millis() < (timeout + 30) * 1000:
                break
            cmd, driver = command(v, url, timeout)
            r = run(cmd, os.path.join(d, f"{v}-live-{i}"), timeout + 15, driver)
            r.update(variant=v, mode="live", run=i, pos=pos)
            rows.append(r)
            subprocess.run("rm -rf /tmp/chr-* /tmp/.org.chromium.* /tmp/.config /tmp/.cache /tmp/.pki", shell=True)
    return s, d, rows


PAGE = b"""<!doctype html><html><head><title>tiny</title></head><body><h1 id=h>hello</h1>
<script>document.getElementById('h').textContent = 'hello ' + [1,2,3].map(x => x * 2).join(',');</script></body></html>"""


def startup(event, ctx):
    """Process start to exit on a page that costs nothing: a local server and one inline script."""
    import http.server

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(PAGE)))
            self.end_headers()
            self.wfile.write(PAGE)

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{srv.server_address[1]}/"
    s = "startup-" + event.get("tag", "0")
    d = os.path.join(WORK, s)
    fresh(d)
    rows = []
    vs = ["dasher", "lightpanda", "chromium"]
    for i in range(event.get("runs", 10)):
        order = vs[i % 3:] + vs[:i % 3]
        for pos, v in enumerate(order):
            for kind in ("version", "tiny-load", "tiny"):
                if kind == "version":
                    cmd = {"dasher": [DASHER, "--version"], "lightpanda": [LP, "version"],
                           "chromium": [CHROME, "--version"]}[v]
                    driver = False
                else:
                    cmd, driver = command(v, url, 20, "load" if kind == "tiny-load" else "idle")
                r = run(cmd, os.path.join(d, f"{v}-{kind}-{i}"), 40, driver)
                r.update(variant=v, mode=kind, run=i, pos=pos)
                rows.append(r)
    srv.shutdown()
    return s, d, rows


def handler(event, ctx):
    try:
        return measure(event, ctx)
    except Exception:
        # An asynchronous invocation has nobody to report to: leave the
        # failure beside where the result would have been, where the driver
        # looks. The runtime loop logs it.
        err = traceback.format_exc()
        phase, tag = event.get("phase"), event.get("tag", "main")
        name = slug(event["site"]["url"]) if "site" in event else f"startup-{tag}"
        try:
            upload(f"{tag}/{phase}/{name}.error.txt", err.encode(), "text/plain")
        except Exception:
            traceback.print_exc()
        raise


def measure(event, ctx):
    phase = event["phase"]
    global _WARM
    if not _WARM:
        t = time.time()
        _warm()
        _WARM = True
        print(f"warmed in {time.time() - t:.1f}s")
    if "chrome_flags" in event:
        CHROME_FLAGS[:] = event["chrome_flags"]
    t0 = time.time()
    s, d, rows = {"live": live, "startup": startup}[phase](event, ctx)
    res = {"phase": phase, "slug": s, "site": event.get("site"), "rows": rows, "sys": sysinfo(),
           "versions": versions(), "elapsed": round(time.time() - t0, 2),
           "request_id": getattr(ctx, "aws_request_id", "")}
    tag = event.get("tag", "main")
    upload(f"{tag}/{phase}/{s}.tar.gz", pack(d), "application/gzip")
    upload(f"{tag}/{phase}/{s}.json", json.dumps(res).encode())
    subprocess.run(["rm", "-rf", d])
    return {"slug": s, "n": len(rows), "elapsed": res["elapsed"]}
