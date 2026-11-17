# Deploying dasher

dasher is one static binary. The same file is the CLI, the HTTP service
(`dasher serve`) and the AWS Lambda runtime (`dasher lambda`, or any copy
named `bootstrap`). The service and the Lambda runtime take the same JSON
[job](#jobs) and answer with the same JSON.

- [Getting the binary](#getting-the-binary)
- [As a container: Cloud Run and others](#as-a-container-cloud-run-and-others)
- [AWS Lambda](#aws-lambda)
- [Jobs](#jobs)
- [Responses](#responses)
- [Sizing and limits](#sizing-and-limits)
- [Security](#security)

## Getting the binary

Each [release](https://github.com/josephjojoe/dasher/releases) carries
static Linux binaries for x86_64 and arm64 (musl, no shared libraries,
so they run on any distribution and in an empty container) and a Lambda
zip for each.

To build it yourself, the [`Dockerfile`](../Dockerfile) builds the static
binary on Alpine and puts it alone on
`gcr.io/distroless/static-debian12:nonroot` (CA certificates, tzdata and
a non-root user). The image runs `dasher serve` on `$PORT` (8080). Choose
the architecture with `--platform`:

```sh
docker build --platform linux/amd64 -t dasher .
docker run --rm -p 8080:8080 dasher
curl -s localhost:8080/health
curl -s localhost:8080/scrape -d '{"url": "https://example.com/", "extract": {"title": "title"}}'
```

The `lambda-zip` stage gives only the binary, named `bootstrap`, so no
musl toolchain is needed on the build machine:

```sh
docker build --platform linux/arm64 --target lambda-zip --output dist .
(cd dist && zip ../dasher-lambda.zip bootstrap LICENSE THIRD-PARTY-LICENSES.txt)
```

Without Docker, on Linux with `musl-tools` installed:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl -p dasher-cli
```

## As a container: Cloud Run and others

The image needs nothing but a port. On Cloud Run:

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
  -d '{"url": "https://example.com/", "extract": {"title": "title"}}'
```

What to know, on Cloud Run or any other container platform:

- **Concurrency.** `dasher serve` runs up to `DASHER_MAX_CONCURRENCY`
  jobs at once (4 by default), each on a thread of its own. Requests past
  that wait for a slot, up to `DASHER_MAX_QUEUE` of them (32), and past
  that get `503` with `Retry-After: 1`. Set the platform's own
  concurrency limit to the same number, so the platform decides what
  waits and when to add instances.
- **Timeouts.** A job's `timeout_ms` (30 s by default) bounds loading,
  settling and scrolling. `DASHER_MAX_TIMEOUT` caps it (300 s). Keep the
  platform's request timeout above both.
- **Shutdown.** On `SIGTERM` dasher stops accepting connections, answers
  `/health` with `503` and `"draining"`, and gives running jobs 8 s
  (`--shutdown-timeout`) to answer. Cloud Run kills an instance 10 s
  after the signal.
- **Health.** `GET /health` answers `200` with the number of jobs
  running and waiting. `/healthz` works too, but Cloud Run reserves
  paths ending in `z`. As a probe:
  `--startup-probe httpGet.path=/health,httpGet.port=8080`.
- **Auth.** Prefer the platform's (`--no-allow-unauthenticated` and IAM
  on Cloud Run). Where the URL has to be public, set `DASHER_AUTH_TOKEN`
  and send `Authorization: Bearer <token>`; without it every caller can
  make the service fetch for them.
- **Logs.** One JSON line per job on stderr (`severity`, `message`,
  `url`, `status`, `queued_ms`, `total_ms` and more), in the shape Cloud
  Logging reads.

Every option is in `dasher serve --help`.

## AWS Lambda

### From a zip

With `dasher-lambda.zip` from a release or from the build above:

```sh
aws lambda create-function --function-name dasher \
  --runtime provided.al2023 --architectures arm64 \
  --handler bootstrap --zip-file fileb://dasher-lambda.zip \
  --memory-size 1024 --timeout 60 \
  --role arn:aws:iam::$ACCOUNT:role/dasher-lambda
```

The role needs only `AWSLambdaBasicExecutionRole`, for logs. Match
`--architectures` to the binary (`arm64` or `x86_64`). The handler name
is not used: the runtime is the `bootstrap` binary itself.

### From the container image

The same image runs on Lambda with its command set to `lambda`:

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

Invoked directly, the event is the job and the result is its response
(with an `error` field if the job failed):

```sh
aws lambda invoke --function-name dasher \
  --cli-binary-format raw-in-base64-out \
  --payload '{"url": "https://example.com/", "extract": {"title": "title"}}' \
  out.json && cat out.json
```

Behind a function URL or API Gateway, `POST` a job as the body and `GET`
for health; the status codes are the HTTP service's.

```sh
aws lambda create-function-url-config --function-name dasher --auth-type AWS_IAM
```

What to know:

- **Timeouts.** A job is cut short 1.5 s before the invocation's
  deadline and answers with the page as far as it got, so set the
  function's `--timeout` a little above the jobs' `timeout_ms`.
- **One job per instance.** Lambda gives an instance one invocation at a
  time. Scale with Lambda's concurrency.
- **Response size.** Lambda limits a response to 6 MB. Ask for `extract`
  rather than `html` on big pages. A response Lambda refuses fails the
  invocation with `Dasher.ResponseRejected`.
- **Memory.** Lambda gives CPU in proportion to memory, and pages are
  mostly CPU, so more memory shortens script-heavy pages well past the
  point where dasher needs the memory itself. 1024 MB is a reasonable
  start.
- **Script cache.** A warm instance keeps the scripts its jobs compiled
  for the invocations that follow (see
  [Sizing and limits](#sizing-and-limits)). `DASHER_SCRIPT_CACHE_DIR`
  keeps them on disk too: in `/tmp` they outlive a restart of the runtime
  in the same sandbox, and in a directory baked into a container image
  every cold start begins with them. A directory that cannot be written
  to is only read.

## Jobs

A job is a JSON object of up to 1 MiB. Only `url` is required. An
unknown field is an error, so a typo does not go unnoticed.

| Field | Default | Meaning |
| --- | --- | --- |
| `url` | (required) | The `http(s)` URL to load. |
| `wait` | `"network-idle"` | When the page counts as ready: `"load"`, `"network-idle"` (no requests for the quiet period), `"network-idle2"` (at most 2), `"dom-quiet"` (no DOM changes for it) or `"settled"` (network idle and DOM quiet). |
| `quiet_ms` | `500` | The quiet period for `wait`. |
| `wait_for` | | Also wait until an element matches this CSS selector. |
| `wait_until` | | Also wait until this JS expression is truthy. |
| `timeout_ms` | `30000` | Time limit for the whole job. Running out is not an error: the page is used as far as it got, with `timed_out: true`. |
| `page_time_budget_ms` | `5000` | The most page time to spend waiting for the page to go quiet. `0` for no limit. See [Budgets](#budgets). |
| `cpu_budget_ms` | `10000` | The most CPU time loading, waiting and scrolling may use. `0` for no limit. |
| `request_budget` | `1000` | The most requests the page may make. `0` for no limit. |
| `scroll` | `false` | `true` or `{"max_scrolls": n}`: scroll to load infinite feeds and lazy content. |
| `html` | `true` unless `extract` is given | Return the page's HTML. |
| `extract` | | Named values to pull out; see below. |
| `console` | `"errors"` | Console messages to return: `"none"`, `"errors"` or `"all"`. |
| `user_agent` | desktop Chrome | `User-Agent` and `navigator.userAgent`. |
| `viewport` | `{"width": 1920, "height": 1080}` | Window size in CSS pixels. |
| `headers` | | Headers for every request, as an object. |
| `cookies` | | Cookies to set for `url` first, in `Set-Cookie` syntax. |
| `block` | | URL patterns to refuse, `*` as a wildcard: `"*://*.doubleclick.net/*"`. |
| `proxy` | | An `http://user:pass@host:port` proxy. See [Security](#security). |
| `realtime` | `false` | Run timers in real time instead of skipping ahead when the page only waits on them. |
| `visibility` | `"loaded"` | What counts as in view, which decides how much lazy content loads: `"loaded"`, `"all"` or `"viewport"`. See [Visibility](../README.md#visibility). |

Each `extract` entry is a CSS selector, giving the text of the first
match (or `null`), or an object:

| Entry | Gives |
| --- | --- |
| `{"selector": "h2"}` | The text of the first match, as a reader sees it, or `null`. |
| `{"selector": "h2", "all": true}` | An array, one entry per match. |
| `{"selector": "a", "attribute": "href"}` | An attribute instead of the text (with `all`, from each match that has it). |
| `{"selector": ".card", "html": "inner"}` | `innerHTML`, or `"outer"` for `outerHTML`. |
| `{"selector": ".card", "count": true}` | The number of matches. |
| `{"eval": "window.__DATA__.items"}` | The JSON value of a JS expression or function; promises are awaited. |

Selectors look into shadow trees.

## Responses

A job that loaded its page answers `200`:

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

- `url` is the final URL, after redirects and script navigations.
- `status` is the site's: a 404 page loads like any other and answers
  `200` here, with `"status": 404`.
- `budget_exhausted` names the budget that ended the page early, if one
  did (`"page_time"`, `"cpu"` or `"requests"`); `settle` (or
  `scroll.stopped`) is then `"budget_exhausted"`. That is a success: the
  page is returned as it stood.
- `html` is there when asked for.
- If an extraction throws, its value is `null` and `extract_errors` says
  why; the rest still come back.
- `errors` lists the page's uncaught exceptions and unhandled rejections.

Failures carry an `error` message and one of these statuses:

| Status | Why |
| --- | --- |
| `400` | The job is invalid: bad JSON, an unknown field, a URL that is not `http(s)`, a bad selector, header or cookie. |
| `401` | `DASHER_AUTH_TOKEN` is set and the request lacks it. |
| `413` | The job is over 1 MiB. |
| `422` | The page went over its JS heap or DOM node limit and was stopped. |
| `502` | The page could not be fetched: DNS, connection, TLS, a blocked URL or a private address. |
| `503` | Too many jobs, or shutting down. Retry. |
| `504` | The document did not answer in time. |

## Sizing and limits

- **Jobs share nothing but the script cache.** Each job gets a browser
  and page of its own (JS heap, DOM, cookies, storage), all freed when
  it answers. The script cache holds compiled code keyed by a hash of
  each script's text, so a later job that loads the same text (the same
  site again, or a library another site uses too) reads it back instead
  of compiling. Entries are made from the text alone and each page reads
  its own copy, so nothing a page does reaches another job.
- **The script cache counts as memory.** It holds up to 32 MiB, text
  included, and stays full once jobs have filled it. `--script-cache
  <MiB>` (`DASHER_SCRIPT_CACHE_MB`) sizes it and `0` turns it off.
- **A page is capped.** Its JS heap at 256 MiB and its DOM at 500,000
  nodes. A page that goes over either is stopped, its memory freed, and
  the job answers `422`, so one runaway page cannot take the instance
  down.
- **What pages cost.** On 336 popular sites that dasher, Lightpanda and
  Chromium all loaded from Lambda, dasher's median peak was 41 MiB and
  nine in ten stayed within 117 MiB (see
  [crates/bench/lambda](../crates/bench/lambda/README.md)). Size an instance
  for its concurrency times a typical page of your sites, plus the
  script cache, with headroom for the heavy ones, and measure.

### Budgets

Besides its time limit, every job is bounded by three budgets, so a page
that would never finish costs a few CPU seconds and not a core until
`timeout_ms`:

- **Page time** (`page_time_budget_ms`, 5 s). dasher's clock skips ahead
  to a page's next timer instead of sleeping. A carousel, ticker or
  countdown changes the page on a timer forever and never lets it go
  quiet, and skipping from tick to tick would turn its 20 ms of work a
  second into a busy loop. Waiting for quiet stops once this much page
  time has been skipped or sat through. Time spent waiting for the
  network or running scripts does not count.
- **CPU** (`cpu_budget_ms`, 10 s): the job thread's CPU time for
  loading, settling and scrolling. It stops scripts that never yield.
- **Requests** (`request_budget`, 1000): for pages that fetch more as
  soon as the last batch arrives.

When one runs out the job answers `200` with the page as it stood and
`budget_exhausted` naming the budget. Extraction still runs: the budgets
are lifted once loading is over. Work queued with `requestIdleCallback`
never holds a page open.

## Security

dasher fetches whatever URL its callers give it and runs that page's
scripts, which can fetch more. Treat it as an open proxy into wherever
it runs.

- **Keep it private.** IAM on Cloud Run and Lambda, or
  `DASHER_AUTH_TOKEN`.
- **Private addresses are refused.** Pages may not connect to loopback
  (`127.0.0.0/8`, `::1`), link-local (`169.254.0.0/16`, where the cloud
  metadata servers are, and `fe80::/10`), private (`10/8`, `172.16/12`,
  `192.168/16` and IPv6 unique local `fc00::/7`), CGNAT (`100.64/10`),
  unspecified, multicast or reserved addresses. The check is made on the
  address dasher connects to, after DNS, for the document, its scripts,
  `fetch`, XHR and every redirect, so a public name that resolves or
  rebinds to one of these is refused too. A job whose document is
  refused answers `502`, naming the address; a refused script or
  `fetch()` fails inside the page like a network error.
- **Metadata endpoints are refused twice.** URLs for `169.254.*`,
  `metadata.google.internal`, AWS's IPv6 endpoint and, on Lambda, the
  Runtime API are refused by pattern as well, even with the address
  check turned off.
- **A proxy sees less.** With a job's `proxy`, the proxy resolves names
  and connects, out of dasher's sight. dasher still refuses a proxy on a
  private address and URLs whose host is a private IP literal, but a
  name the proxy resolves to a private address gets through. Use proxies
  that refuse private destinations themselves.
- **Turning the check off** (`--allow-private-addresses`, or
  `DASHER_ALLOW_PRIVATE_ADDRESSES=true` for both `serve` and `lambda`)
  is for scraping servers on localhost or a private network, as in
  tests. Do not do it where callers are not trusted.
- **Limit egress anyway** where the service could reach a private
  network (a VPC connector with no private routes, a Lambda outside a
  VPC), as a second line of defence.

The CLI makes none of these checks: it is run by the person who chose
the URL.
