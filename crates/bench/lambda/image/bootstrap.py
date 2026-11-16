"""Minimal Lambda runtime loop (the image is plain Debian, not a Lambda base image)."""
import json, os, sys, time, traceback, urllib.request
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import handler
API = os.environ["AWS_LAMBDA_RUNTIME_API"]
BASE = f"http://{API}/2018-06-01/runtime"

class Ctx:
    def __init__(self, rid, deadline_ms):
        self.aws_request_id = rid; self.deadline_ms = deadline_ms
    def get_remaining_time_in_millis(self):
        return int(self.deadline_ms - time.time() * 1000)

def post(path, obj):
    req = urllib.request.Request(BASE + path, data=json.dumps(obj).encode(), method="POST")
    urllib.request.urlopen(req).read()

while True:
    r = urllib.request.urlopen(BASE + "/invocation/next")
    rid = r.headers["Lambda-Runtime-Aws-Request-Id"]
    ctx = Ctx(rid, int(r.headers["Lambda-Runtime-Deadline-Ms"]))
    try:
        out = handler.handler(json.loads(r.read()), ctx)
        post(f"/invocation/{rid}/response", out)
    except Exception as e:
        # The Runtime API keeps the error for the caller only, and an
        # asynchronous invocation has none.
        traceback.print_exc()
        sys.stderr.flush()
        post(f"/invocation/{rid}/error", {"errorMessage": str(e), "errorType": type(e).__name__,
                                          "stackTrace": traceback.format_exc().splitlines()})
