# Deploying dasher

dasher is one static binary, and the same binary is the CLI, the HTTP
service (`dasher serve`) and the AWS Lambda runtime (`dasher lambda`, or
any copy named `bootstrap`). Both kinds of deployment take the same JSON
[job](#jobs) and answer with the same JSON.

- [Build](#build)
- [Cloud Run](#cloud-run)
- [AWS Lambda](#aws-lambda)
- [Jobs](#jobs)
- [Sizing and limits](#sizing-and-limits)
- [Security](#security)

## Build

The [`Dockerfile`](../Dockerfile) builds a static musl binary on Alpine
and puts it alone on `gcr.io/distroless/static-debian12:nonroot`
(CA certificates, tzdata and a non-root user; about 2 MB). The image runs
`dasher serve` on `$PORT` (8080). Pick the architecture with
`--platform`:

```sh
docker build --platform linux/amd64 -t dasher .
docker run --rm -p 8080:8080 dasher
curl -s localhost:8080/health
curl -s localhost:8080/scrape -d '{"url": "https://example.com/", "extract": {"title": "h1"}}'
```

For a Lambda zip, the `lambda-zip` stage gives just the binary, named
`bootstrap`. No musl toolchain is needed on the build machine:

```sh
docker build --platform linux/arm64 --target lambda-zip --output dist .
(cd dist && zip ../dasher-lambda.zip bootstrap)
```

Without Docker, on Linux with `musl-tools` installed:
`cargo build --release --locked --target x86_64-unknown-linux-musl -p dasher-cli`.

## Cloud Run

Push the image to Artifact Registry and deploy it:

```sh
REGION=europe-west1
IMAGE=$REGION-docker.pkg.dev/$PROJECT/containers/dasher
docker build --platform linux/amd64 -t $IMAGE . && docker push $IMAGE

gcloud run deploy dasher --image $IMAGE --region $REGION \
  --memory 1Gi --cpu 1 \
  --concurrency 4 --set-env-vars DASHER_MAX_CONCURRENCY=4 \
  --timeout 300 \
  --no-allow-unauthenticated
```

Then call it with an identity token:

```sh
URL=$(gcloud run services describe dasher --region $REGION --format 'value(status.url)')
curl -s "$URL/scrape" \
  -H "Authorization: Bearer $(gcloud auth print-identity-token)" \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://example.com/", "extract": {"title": "h1"}}'
```

Notes:

- **Concurrency.** Set Cloud Run's `--concurrency` to dasher's
  `DASHER_MAX_CONCURRENCY` so Cloud Run, not dasher, decides what waits
  and when to add instances. Requests past dasher's cap wait for a slot
  (up to `DASHER_MAX_QUEUE`, 32 by default) and past that get `503` with
  `Retry-After: 1`.
- **Timeouts.** A job's `timeout_ms` (30 s by default) bounds loading,
  settling and scrolling; dasher caps it at `DASHER_MAX_TIMEOUT` (300 s).
  Keep Cloud Run's `--timeout` above both.
- **Shutdown.** Cloud Run sends `SIGTERM` and kills the instance 10 s
  later. dasher stops accepting connections, reports `503 draining` from
  `/health`, and gives running jobs 8 s (`--shutdown-timeout`) to answer.
- **Health.** `GET /health` (not `/healthz`: Cloud Run reserves paths
  ending in `z`) can be the startup and liveness probe:
  `--startup-probe httpGet.path=/health,httpGet.port=8080`.
- **Auth.** Prefer Cloud Run's IAM (`--no-allow-unauthenticated`). For
  a public URL, set `DASHER_AUTH_TOKEN` (from Secret Manager with
  `--set-secrets DASHER_AUTH_TOKEN=dasher-token:latest`) and send
  `Authorization: Bearer <token>`.
- **CPU.** Jobs run inside their requests, so the default
  request-based billing is right. With 2+ vCPUs, jobs run in parallel on
  them: each job has a thread of its own.
- **Logs.** One JSON line per job on stderr (`severity`, `message`,
  `url`, `status`, `total_ms`, …), which Cloud Logging parses.

Other flags: `dasher serve --help`.

## AWS Lambda

### From a zip (`provided.al2023`)

Build `dasher-lambda.zip` as [above](#build), then:

```sh
aws lambda create-function --function-name dasher \
  --runtime provided.al2023 --architectures arm64 \
  --handler bootstrap --zip-file fileb://dasher-lambda.zip \
  --memory-size 1024 --timeout 60 \
  --role arn:aws:iam::$ACCOUNT:role/dasher-lambda
```

The role needs only `AWSLambdaBasicExecutionRole` (for logs). Match
`--architectures` to the `--platform` the zip was built for
(`arm64` ↔ `linux/arm64`, `x86_64` ↔ `linux/amd64`). The handler name is
not used: the runtime is the `bootstrap` binary.

### From the container image

The same image works on Lambda with its command set to `lambda`:

```sh
aws ecr create-repository --repository-name dasher
IMAGE=$ACCOUNT.dkr.ecr.$REGION.amazonaws.com/dasher:latest
docker build --platform linux/arm64 --provenance=false -t $IMAGE . && docker push $IMAGE

aws lambda create-function --function-name dasher \
  --package-type Image --code ImageUri=$IMAGE \
  --image-config '{"Command": ["lambda"]}' \
  --architectures arm64 --memory-size 1024 --timeout 60 \
  --role arn:aws:iam::$ACCOUNT:role/dasher-lambda
```

### Invoking

Directly, the event is the job and the response its JSON (with an
`error` field if the job failed):

```sh
aws lambda invoke --function-name dasher \
  --cli-binary-format raw-in-base64-out \
  --payload '{"url": "https://example.com/", "extract": {"title": "h1"}}' \
  out.json && cat out.json
```

Over HTTP, through a function URL (or API Gateway): `POST` a job as
the body; `GET` reports health. Status codes match the HTTP service's.

```sh
aws lambda create-function-url-config --function-name dasher --auth-type AWS_IAM
```

Notes:

- **Timeouts.** Each job is cut short to finish 1.5 s before the
  invocation's deadline, returning the page as far as it got, so set the
  function's `--timeout` a little above the jobs' `timeout_ms`.
- **One job per instance.** Lambda sends an instance one invocation at a
  time; scale with concurrency, not threads.
- **Payload size.** Responses are limited to 6 MB. Ask for `extract`
  rather than `html` on big pages. A response Lambda refuses fails the
  invocation with `Dasher.ResponseRejected`.
- **Memory.** More memory also buys more CPU on Lambda, which shortens
  script-heavy pages; 1024 MB is a reasonable start.

## Jobs

A job is a JSON object. Only `url` is required; unknown fields are an
error, so typos don't go unnoticed.

| Field | Default | Meaning |
| --- | --- | --- |
| `url` | (required) | The `http(s)` URL to load. |
| `wait` | `"settled"` | When the page counts as loaded: `"load"`, `"network-idle"`, `"network-idle2"` (≤ 2 requests), `"dom-quiet"`, or `"settled"` (network idle and DOM quiet). |
| `quiet_ms` | `500` | The quiet period for `wait`. |
| `wait_for` | | Also wait until an element matches this CSS selector. |
| `wait_until` | | Also wait until this JS expression is truthy. |
| `timeout_ms` | `30000` | Time limit for the whole job. Running out is not an error: the page is used as far as it got, with `timed_out: true`. |
| `page_time_budget_ms` | `5000` | The most page time to spend waiting for the page to go quiet: time the clock skips ahead, and time the page sits idle on its timers (not running scripts, nothing in flight). Pages that update on a timer forever never go quiet; once this is spent the quiet periods are given up. `0` for no limit. See [Budgets](#budgets). |
| `cpu_budget_ms` | `10000` | The most CPU time loading, waiting and scrolling may use. `0` for no limit. |
| `request_budget` | `1000` | The most requests the page may make: document, scripts, `fetch()` and XHR. `0` for no limit. |
| `scroll` | `false` | `true` or `{"max_scrolls": n}`: scroll to load infinite feeds and lazy content. |
| `html` | `true` unless `extract` is given | Return the page's HTML. |
| `extract` | | Named values to pull out; see below. |
| `console` | `"errors"` | Console messages to return: `"none"`, `"errors"` or `"all"`. |
| `user_agent` | desktop Chrome | `User-Agent` and `navigator.userAgent`. |
| `viewport` | `{"width": 1920, "height": 1080}` | Window size in CSS pixels. |
| `headers` | | Headers for every request, as an object. |
| `cookies` | | Cookies set for `url` first, in `Set-Cookie` syntax. |
| `block` | | URL patterns to refuse, `*` as a wildcard: `"*://*.doubleclick.net/*"`. |
| `proxy` | | An `http://user:pass@host:port` proxy. It resolves host names itself, so the private address check sees less; see [Security](#security). |
| `realtime` | `false` | Run timers in real time instead of skipping ahead when the page only waits on them. |
| `visibility` | `"loaded"` | What counts as in view, which decides how much lazy content loads: `"loaded"` (the page as it stood at load; what it adds later is below the fold until `scroll` reveals it), `"all"` (everything, always: the most lazy content, but a page that keeps loading more runs to the timeout) or `"viewport"` (only what is scrolled to: the least work and content). See [Visibility](../README.md#visibility). |

Each `extract` entry is either a CSS selector, giving the text of the
first match (or `null`), or an object:

| Entry | Gives |
| --- | --- |
| `{"selector": "h2"}` | The text of the first match, as a reader sees it, or `null`. |
| `{"selector": "h2", "all": true}` | An array, one entry per match. |
| `{"selector": "a", "attribute": "href"}` | An attribute instead of the text (with `all`, from each match that has it). |
| `{"selector": ".card", "html": "inner"}` | `innerHTML` (or `"outer"` for `outerHTML`). |
| `{"selector": ".card", "count": true}` | The number of matches. |
| `{"eval": "window.__DATA__.items"}` | The JSON value of a JS expression or function; promises are awaited. |

A successful job answers `200`:

```json
{
  "url": "https://example.com/final",
  "status": 200,
  "title": "Example",
  "timed_out": false,
  "settle": "settled",
  "scroll": null,
  "budget_exhausted": null,
  "requests": 42,
  "extracted": { "title": "Example Domain" },
  "console": [{ "level": "error", "text": "…" }],
  "errors": [{ "kind": "uncaught", "message": "…", "stack": "…" }],
  "timings": { "queued_ms": 0, "navigation_ms": 812, "extract_ms": 1, "total_ms": 815 }
}
```

`status` is the site's status: a 404 page loads like any other.
`budget_exhausted` names the budget that ended the page early, if one
did (`"page_time"`, `"cpu"` or `"requests"`); `settle` (or
`scroll.stopped`) is then `"budget_exhausted"`. That is a success, not a
timeout: the page is returned as it stood. `requests` counts the
requests the page made.
`html` is there when asked for. If an extraction throws, its value is
`null` and `extract_errors` says why; the rest still come back. `errors`
lists the page's uncaught exceptions and unhandled rejections.

Failures have an `error` message and these statuses:

| Status | Why |
| --- | --- |
| `400` | The job is invalid: bad JSON, unknown field, not an `http(s)` URL, bad selector, header or cookie. |
| `401` | `DASHER_AUTH_TOKEN` is set and the request lacks it. |
| `413` | The job is over 1 MiB. |
| `422` | The page went over its JS heap or DOM node limit and was stopped. |
| `502` | The page could not be fetched (DNS, connection, TLS, blocked URL or private address). |
| `503` | Too many jobs, or shutting down; retry. |
| `504` | No response for the document in time. |

## Sizing and limits

- Each job gets its own browser and page (its own JS heap, DOM, cookies
  and storage), all freed when it answers. Nothing is shared between
  jobs.
- A page's JS heap is capped at 256 MiB and its DOM at 500,000 nodes. A
  page that goes over either is stopped and its memory freed, and the job
  fails with `422`, so one runaway page cannot take down the instance.
- Measured on the static musl build (see `crates/bench/README.md`), a
  ready process idles at about 4 MiB RSS and a job on a React SPA peaks
  near 12 MiB; a 1.6 MB, 75,000-node page peaks near 17 MiB. Size memory
  for your concurrency times a typical page of your sites plus a few MB,
  with headroom for heavy pages, and measure.
- A page that spins in JavaScript is stopped by its CPU budget (10 s by
  default) or its time limit, and holds up only its own job.

### Budgets

Every job is bounded by three budgets besides its time limit, so a page
that would never finish costs a few CPU seconds rather than a core until
`timeout_ms`:

- **Page time** (`page_time_budget_ms`, 5 s). Under the default virtual
  clock dasher skips ahead to a page's next timer instead of sleeping. A
  carousel, ticker or countdown that changes the page on a timer forever
  never lets it go quiet, and skipping from tick to tick would turn its
  20 ms of work a second into a busy loop. Settling stops once it has
  skipped (or sat idle for) this much page time. Waiting for the network
  and running scripts do not count.
- **CPU** (`cpu_budget_ms`, 10 s): the job thread's CPU time for loading,
  settling and scrolling. It catches scripts that never yield.
- **Requests** (`request_budget`, 1000): for pages that fetch "more
  stories" as soon as the last batch arrives. The heaviest sites
  measured make about 400.

When one runs out, the job answers `200` with the page as it stood and
`budget_exhausted` naming the budget. Extraction still runs: the budgets
are lifted once loading is over. `requestIdleCallback` work never holds a
page open. On 142 recorded real sites, the budgets took the CPU time of a
sample from 844 s to about 200 s; see `crates/bench/README.md`.

## Security

dasher fetches whatever URLs its callers give it, and runs those pages'
scripts, which can fetch more. Treat it as an open proxy into wherever it
runs:

- Keep it private: IAM on Cloud Run and Lambda, or `DASHER_AUTH_TOKEN`.
- Pages may not connect to private addresses: loopback (`127.0.0.0/8`,
  `::1`), link-local (`169.254.0.0/16`, where the cloud metadata servers
  are, and `fe80::/10`), private (`10/8`, `172.16/12`, `192.168/16` and
  IPv6 unique local `fc00::/7`, which holds AWS's IPv6 metadata
  endpoint), CGNAT (`100.64/10`), unspecified (`0.0.0.0`, `::`),
  multicast and reserved. That includes the Lambda Runtime API, which is
  on localhost. The check is made on the address dasher connects to,
  after DNS, for the page, its scripts, `fetch`/XHR and every redirect,
  so a public name that resolves or rebinds to one of these is refused
  too. A job whose page is refused gets `502` with an error saying which
  address was refused; a refused script or `fetch()` fails inside the
  page like a network error.
- On top of that, URLs for the metadata endpoints (`169.254.*`,
  `metadata.google.internal`, AWS's IPv6 endpoint) and, on Lambda, the
  Runtime API are always refused by pattern, even with the check above
  turned off.
- A job's `proxy` changes what can be checked: the proxy resolves host
  names and connects to them, out of dasher's sight. dasher still
  refuses a proxy on a private address and target URLs whose host is a
  private IP literal, but a name that the proxy resolves to a private
  address gets through. Use proxies that refuse private destinations
  themselves.
- Turning the check off (`--allow-private-addresses` or
  `DASHER_ALLOW_PRIVATE_ADDRESSES=true`, read by both `dasher serve`
  and `dasher lambda`) is for scraping servers on localhost or a private
  network, such as when testing. Don't do it where callers aren't
  trusted.
- Still limit egress if the service can reach private networks (a VPC
  connector with no private routes, or a Lambda outside a VPC), as a
  second line of defence.
