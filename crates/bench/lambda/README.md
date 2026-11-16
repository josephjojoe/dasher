# dasher, Lightpanda and Chromium on AWS Lambda

A live comparison on real sites, in the place dasher is built for: one
Lambda invocation per site, with every browser run back to back in the
same sandbox, so each site's numbers come from one machine, one network
path and one few-minute window.

- [Results, 2026-11-16](#results-2026-11-16)
- [Method](#method)
- [What this does not show](#what-this-does-not-show)
- [Running it](#running-it)

The raw report is [`results/2026-11-16/report.txt`](results/2026-11-16/report.txt)
and the per-site table [`results/2026-11-16/sites.csv`](results/2026-11-16/sites.csv).

## Results, 2026-11-16

dasher 0.1.0, Lightpanda `1.1.0-nightly.12181+2519e1716` and Chromium's
headless shell 156.0.8078.79, on 492 sites, three runs each, on arm64
Lambda with 3008 MB (2 vCPUs) in us-east-1. Each tool ran with its
defaults and was asked for the page's HTML once the network went idle.

### What each could load

| Per site, majority of three runs | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| Loaded | 408 | 393 | 359 |
| Answered with a bot wall | 62 | 76 | 110 |
| HTTP error page | 7 | 7 | 10 |
| Empty page | 10 | 14 | 9 |
| No document (connection, TLS, timeout) | 5 | 2 | 4 |
| Crashed or killed | 0 | 0 | 0 |

All three loaded 340 sites and none loaded 69. Requests came from AWS
addresses, which bot protection treats harshly: 51 sites answered every
tool with a challenge. dasher got past the most: 25 sites loaded for it
and not for Lightpanda, 10 the other way. Chromium's headless shell was
refused most often.

dasher's five failures: four sites that Lightpanda could not load
either (gnu.org, search-load.com, bmvm.bihar.gov.in and a login redirect
at jvsonline.fr that could not be connected to) and asnet.jp (the server
offers only finite-field Diffie-Hellman key exchange, which rustls does
not implement). Seven of its ten empty pages were empty for all three.
Of the others, baccredomatic.com spent the whole CPU budget before it
showed anything, and dashboard.composio.dev and student.amplify.com
came back with a few characters. duolingo.com sent dasher to its
"browser not supported" page, a 404: it wants `WebAssembly`.

### What a page cost

The 340 sites all three loaded:

| | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| CPU per page, median | 0.87 s | 0.93 s | 3.71 s |
| CPU per page, mean | 1.97 s | 1.76 s | 6.40 s |
| CPU, 90th percentile | 5.73 s | 4.71 s | 14.56 s |
| Wall time, median | 2.12 s | 2.69 s | 5.32 s |
| Wall time, mean | 3.27 s | 2.90 s | 6.69 s |
| Peak memory, median | 41 MiB | 92 MiB | 387 MiB |
| Peak memory, 90th percentile | 126 MiB | 208 MiB | 671 MiB |
| Peak memory, largest | 441 MiB | 629 MiB | 1172 MiB |

| Ratio over those sites | total | median site |
| --- | ---: | ---: |
| dasher / Lightpanda, CPU | 1.12× | 0.87× |
| dasher / Lightpanda, wall time | 1.13× | 0.88× |
| dasher / Lightpanda, memory | 0.50× | 0.40× |
| dasher / Chromium, CPU | 0.31× | 0.23× |
| dasher / Chromium, wall time | 0.49× | 0.41× |
| dasher / Chromium, memory | 0.13× | 0.10× |
| Lightpanda / Chromium, CPU | 0.28× | 0.26× |
| Lightpanda / Chromium, memory | 0.26× | 0.23× |

Read as:

- **Memory is where dasher is ahead**: four tenths of Lightpanda's peak
  on the median site and a tenth of Chromium's. It used less than
  Lightpanda on 331 of the 340 sites and less than Chromium on all of
  them.
- **CPU is close to Lightpanda's**: less on the median site and 12%
  more in total, lower on 186 sites, within 10% on 60 and higher on
  94. Its costly pages cost more: 5.7 s at the 90th percentile against
  4.7 s. Both use between a quarter and a third of Chromium's.
- **Wall time follows the same pattern.** dasher finished sooner than
  Lightpanda on the median site, and later in total: Lightpanda stops
  waiting 5 s after it starts (88 sites reached that cap), where dasher
  keeps going until the page is quiet or a budget runs out. Six dasher
  runs in 1,476 took 25 s or more.
- **Per-run spread is small**: over a site's three runs, CPU varied by
  4% of the median for dasher and Lightpanda (17% and 13% at the 90th
  percentile), 7% for Chromium.

### The same content

Loading a page is not the same work if the result differs, so the
comparison is repeated on sites where it does not. A site counts as
*same content* when dasher's and Lightpanda's visible text each share at
least 0.9 of their distinct words with Chromium's (Jaccard similarity of
the word sets).

| 251 same-content sites | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| CPU, total | 400 s | 375 s | 1411 s |
| CPU, median | 0.72 s | 0.73 s | 2.99 s |
| Peak memory, median | 33 MiB | 82 MiB | 365 MiB |

dasher used 1.07× Lightpanda's CPU on these (median site 0.84×) and
0.28× Chromium's.

dasher against Lightpanda alone, on sites where their texts are at
least 0.9 similar, their request counts within 25%, either spent more
than a CPU second and dasher ended on no budget (105 sites): dasher used
1.18× the CPU (median site 1.09×) and 0.55× the memory. This is the
closest the run comes to the two doing the same work, and there
dasher's interpreter (QuickJS) is behind Lightpanda's V8.

On light pages (Chromium under 2 CPU s, 94 sites) the median was 0.11 s
and 12 MiB for dasher, 0.18 s and 46 MiB for Lightpanda, 1.02 s and
272 MiB for Chromium.

### How much of the page came back

Against Chromium's text, on the sites both loaded:

| | dasher | Lightpanda |
| --- | ---: | ---: |
| Sites compared | 345 | 349 |
| Median similarity | 0.997 | 0.997 |
| At least 0.9 similar | 270 | 269 |
| Under 0.5 similar | 20 | 26 |
| Under half Chromium's text | 10 | 14 |

Chromium against itself, run to run, was at least 0.9 similar on 318 of
345 sites: the rest serve different text on each load.

The ten sites where dasher kept under half of Chromium's text, with the
page error behind it where there was one (see
[COMPATIBILITY.md](../../../COMPATIBILITY.md)):

- mongodb.com and seller.flipkart.com: a 2D context asked of a canvas
  that is not in the document, which dasher answers with `null`.
- google.com/maps: `XPathResult` is undefined.
- target.com: `WebAssembly` is undefined.
- disneyplus.com and sooka.my: React gave up hydrating the page (its
  error 423), on sooka.my after a `TypeError` in the page's own bundle.
  Lightpanda kept as little of disneyplus.com.
- word.cloud.microsoft: 63 characters and no page error. Lightpanda got
  the same 63.
- amazon.com: the server answered dasher and Lightpanda with a
  "Continue shopping" interstitial under status 200, which the outcome
  rules here do not take for a bot wall, and Chromium with the home
  page.
- digikala.com: the page is built by scripts after load, so under the
  default visibility policy its shelves are below the fold and never
  load. `--visibility all` or `--scroll` returns them.
- yahoo.com: Yahoo sends a fuller page, about four times the text in
  the HTML, to a user agent that names `HeadlessChrome` (Chromium's
  here) or Lightpanda, and the short one to dasher, which names Chrome.
  Against Chrome with an ordinary user agent, from a home connection,
  what dasher lacks is the rest of the feed, which loads on coming into
  view (as on digikala.com), and the ad frames.

The page errors are this run's. The digikala.com and yahoo.com
explanations were worked out from a home connection.

### Limits

- dasher: 39 runs of 1,476 ended on the CPU budget (10 s) and 3 on the
  request budget; 3 reached the 30 s time limit. Fourteen sites ended
  on a budget on most runs (among them bloomberg.com, homedepot.com,
  hubspot.com, huffpost.com, salesforce.com). A budget is a success:
  the page is returned as it stood.
- Lightpanda: 272 runs stopped at its 5 s wait cap. With the cap at
  30 s it used 1.17× the CPU, kept over 10% more text on 15 sites, and
  was at least 0.9 similar to Chromium's text on 271 sites, against 269.
- Chromium: on 191 sites it was still making requests 5 s after `load`
  and was read as it stood.

### Starting up

Process start to exit on a page that costs nothing (one inline script,
served from the sandbox itself), ten sandboxes, ten runs each, medians
([`results/2026-11-16/startup.txt`](results/2026-11-16/startup.txt)):

| | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| Print the version: wall | 10 ms | 10 ms | 31 ms |
| Load the page: wall | 21 ms | 41 ms | 324 ms |
| Load the page: CPU | 11 ms | 39 ms | 270 ms |
| Load the page: peak memory | 7.1 MiB | 22.6 MiB | 195 MiB |

Wall times are in steps of 10 ms, the runner's sampling period.
Waiting for network idle on top of `load` cost dasher and Lightpanda
nothing and Chromium 500 ms.

### Cost

On Lambda the bill is wall time times configured memory. At the 3008 MB
these ran with, the mean page of the 340 comes to $128 per million for
dasher, $114 for Lightpanda and $262 for Chromium (us-east-1 arm64
prices, request fee included). dasher's peak memory would allow a far
smaller function, but Lambda gives CPU in proportion to memory, and
that trade was not measured here.

## Method

- **Sites.** 272 origins drawn at random from the Chrome UX Report's
  top-sites list, by rank (53 from the top 1,000, 111 from the next
  9,000, 108 from the next 90,000), after removing adult, gambling and
  piracy sites by name, and the 224 sites of
  [`../compare/sites.txt`](../compare/sites.txt), chosen by hand to
  cover news, shopping, documentation, SaaS, government and web apps.
  Four are in both. 500 were measured; eight were dropped afterwards
  when their pages turned out to be gambling, piracy or unidentifiable,
  leaving the 492 in [`sites.json`](sites.json).
- **One sandbox per site.** A Lambda function built from
  [`image/`](image) takes a site and runs five variants on it, three
  rounds, starting each site's rotation at a different variant (from a
  hash of its name), so over the sites every variant runs in every
  position. Besides the three tools, `lightpanda-30s` raises
  Lightpanda's wait cap and `chromium-noimg` turns Chromium's images
  off (0.87× its CPU, 0.94× its memory). Median CPU by position in the
  round shows no trend (dasher's five positions: 0.90, 0.79, 0.89, 0.79
  and 0.89 s, each over a different set of sites).
- **Commands.**
  - `dasher --json -t 30 URL`: defaults, which wait for the network to
    be idle for 500 ms within the page-time, CPU and request budgets.
  - `lightpanda fetch --dump html --json --metrics --wait-until networkidle URL`:
    defaults otherwise, among them a 5 s cap on waiting.
  - Chromium: `chromium-headless-shell` started for each page and
    driven over `--remote-debugging-pipe` by [`image/cdp.py`](image/cdp.py),
    which does what Puppeteer's `networkidle0` does: wait for `load`,
    then for 500 ms with no request in flight, then read
    `document.documentElement.outerHTML`. A page still busy 5 s after
    `load` is read as it stands. Flags: `--no-sandbox --disable-gpu
    --no-zygote` (Lambda forbids the zygote's namespaces) and a
    1920×1080 window.
- **Measuring.** [`image/runner.c`](image/runner.c), a static launcher,
  starts each command in its own session and reports its `rusage` and,
  sampled every 10 ms from `/proc`, the CPU of every process in the
  session and their summed memory. dasher and Lightpanda are one
  process each: CPU is user plus system time and memory is peak RSS,
  both from the kernel. Chromium is five processes at once on the
  median page: CPU is the sum over all of them and memory is the peak
  of their summed PSS, which counts shared pages once (summed RSS is
  1.54× that). The Python driver is left out. A process that outlives
  its 45 s is killed; none was.
- **Warm-up.** Each sandbox reads every binary once and starts each
  tool once before measuring, so no run pays for the image's lazy
  loading.
- **Hardware.** Lambda's arm64 fleet is mixed: 364 of these sandboxes
  reported Neoverse N1 cores, 105 Neoverse V1 and 23 Neoverse V2. Ratios
  are within a sandbox; absolute times are over that mix.
- **Outcomes.** A run *loaded* if it exited 0 with a status under 400
  and at least 40 characters of visible text that are not a bot wall's.
  It is *blocked* on a challenge status (401, 403, 429, 503 and a few
  others) with a wall's wording or under 1,500 characters, on a wall's
  wording with under 1,200 characters, or when Lightpanda itself reports
  a bot challenge. A site's outcome is its runs' majority. See
  [`lib.py`](lib.py).

## What this does not show

- **Correctness beyond visible text.** Two pages with the same words
  can differ in structure, attributes and links.
- **Fairness to a renderer.** Chromium lays out and paints, loads
  images, fonts and stylesheets, and runs in five processes. It is here
  as the reference for content and as what a scraper would otherwise
  run, not as a like-for-like engine.
- **Sites behind bot protection.** Between an eighth and a fifth of
  the list answered with a wall, depending on the tool. Which tool gets
  through depends on its TLS and HTTP fingerprint and on the address it
  calls from more than on anything measured here.
- **Other hardware.** x86_64, and CPUs with more or less cache, were
  not measured.
- **One day.** Sites change by the hour; three runs minutes apart
  bound the noise within a site, not across days.

## Running it

About 500 invocations at 3008 MB, a minute of work each: this run cost
about $2. AWS CLI v2, Docker and Python 3 with `boto3`.

```sh
export AWS_DEFAULT_REGION=us-east-1
ACCOUNT=$(aws sts get-caller-identity --query Account --output text)
export BENCH_BUCKET=dasher-bench-$ACCOUNT
REPO=$ACCOUNT.dkr.ecr.$AWS_DEFAULT_REGION.amazonaws.com/dasher-bench
cd crates/bench/lambda

# dasher, static, for arm64 (from the repository's own Dockerfile).
docker build --platform linux/arm64 --target lambda-zip --output type=local,dest=out ../../..
mkdir -p image/bin && cp out/bootstrap image/bin/dasher

# A bucket for results, a role that may write to it, the image, the function.
aws s3 mb s3://$BENCH_BUCKET
aws iam create-role --role-name dasher-bench-role --assume-role-policy-document \
  '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}'
aws iam attach-role-policy --role-name dasher-bench-role \
  --policy-arn arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole
aws iam put-role-policy --role-name dasher-bench-role --policy-name s3put --policy-document \
  '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:PutObject"],"Resource":"arn:aws:s3:::'$BENCH_BUCKET'/*"}]}'
aws ecr create-repository --repository-name dasher-bench
aws ecr get-login-password | docker login --username AWS --password-stdin ${REPO%/*}
docker build --platform linux/arm64 --provenance=false -t ${REPO}:latest image && docker push ${REPO}:latest
aws lambda create-function --function-name dasher-bench --package-type Image \
  --code ImageUri=${REPO}:latest --architectures arm64 --memory-size 3008 --timeout 900 \
  --environment "Variables={BENCH_BUCKET=$BENCH_BUCKET}" \
  --role arn:aws:iam::${ACCOUNT}:role/dasher-bench-role
# Lambda must not run a failed asynchronous invocation again by itself:
# drive.py prints the error and invokes it again, once.
aws lambda put-function-event-invoke-config --function-name dasher-bench --maximum-retry-attempts 0

# Run, fetch, report.
python3 drive.py live r1
python3 drive.py startup s1
aws s3 sync s3://$BENCH_BUCKET data --quiet
python3 extract.py r1
python3 report.py r1 > report.txt
python3 startup.py s1

# Remove everything.
aws lambda delete-function --function-name dasher-bench
aws logs delete-log-group --log-group-name /aws/lambda/dasher-bench
aws ecr delete-repository --repository-name dasher-bench --force
aws iam delete-role-policy --role-name dasher-bench-role --policy-name s3put
aws iam detach-role-policy --role-name dasher-bench-role \
  --policy-arn arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole
aws iam delete-role --role-name dasher-bench-role
aws s3 rb s3://$BENCH_BUCKET --force
```

The image is Debian trixie because Lightpanda's release binary needs
glibc 2.38, newer than Lambda's managed runtimes have; its entrypoint is
a small runtime loop of its own ([`image/bootstrap.py`](image/bootstrap.py)).
The `Dockerfile` takes Lightpanda's nightly; pass
`--build-arg LIGHTPANDA_URL=…` to pin a release.
