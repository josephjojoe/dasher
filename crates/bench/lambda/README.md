# dasher, Lightpanda and Chromium on AWS Lambda

A live comparison on real sites, in the place dasher is built for: one
Lambda invocation per site, with every browser run back to back in the
same sandbox, so each site's numbers come from one machine, one network
path and one few-minute window.

- [Results, 2026-11-15](#results-2026-11-15)
- [Method](#method)
- [What this does not show](#what-this-does-not-show)
- [Running it](#running-it)

The raw report is [`results/2026-11-15/report.txt`](results/2026-11-15/report.txt)
and the per-site table [`results/2026-11-15/sites.csv`](results/2026-11-15/sites.csv).

## Results, 2026-11-15

dasher 0.1.0, Lightpanda `1.1.0-nightly.12173+18ff596e6` and Chromium's
headless shell 156.0.8078.79, on 492 sites, three runs each, on arm64
Lambda with 3008 MB (2 vCPUs) in us-east-1. Each tool ran with its
defaults and was asked for the page's HTML once the network went idle.

### What each could load

| Per site, majority of three runs | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| Loaded | 372 | 390 | 355 |
| Answered with a bot wall | 93 | 76 | 111 |
| HTTP error page | 8 | 8 | 12 |
| Empty page | 9 | 16 | 9 |
| No document (connection, TLS, timeout) | 10 | 2 | 5 |
| Crashed or killed | 0 | 0 | 0 |

All three loaded 328 sites and none loaded 85. Requests came from AWS
addresses, which bot protection treats harshly, so about one site in
five answered every tool with a challenge. Lightpanda got past more of
them: 29 sites loaded for it and not for dasher, 11 the other way.
Chromium's headless shell was refused most often.

dasher's ten failures: five sites that Lightpanda or Chromium could not
reach either (gnu.org, search-load.com, rouydad24.ir, washingtonpost.com,
bmvm.bihar.gov.in), asnet.jp (the server offers only finite-field
Diffie-Hellman key exchange, which rustls does not implement),
bestbuy.com (the request was reset; it loads from a home connection),
two documents that did not answer in 30 s (baccredomatic.com, glami.gr)
and one login redirect that could not be connected to.

### What a page cost

The 328 sites all three loaded:

| | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| CPU per page, median | 0.94 s | 1.05 s | 3.74 s |
| CPU per page, mean | 1.86 s | 1.80 s | 6.61 s |
| CPU, 90th percentile | 5.33 s | 4.83 s | 14.95 s |
| Wall time, median | 2.04 s | 2.67 s | 5.42 s |
| Wall time, mean | 3.10 s | 2.87 s | 6.67 s |
| Peak memory, median | 39 MiB | 94 MiB | 387 MiB |
| Peak memory, 90th percentile | 120 MiB | 209 MiB | 671 MiB |
| Peak memory, largest | 437 MiB | 629 MiB | 1233 MiB |

| Ratio over those sites | total | median site |
| --- | ---: | ---: |
| dasher / Lightpanda, CPU | 1.04× | 1.03× |
| dasher / Lightpanda, wall time | 1.08× | 0.89× |
| dasher / Lightpanda, memory | 0.49× | 0.41× |
| dasher / Chromium, CPU | 0.28× | 0.24× |
| dasher / Chromium, wall time | 0.46× | 0.39× |
| dasher / Chromium, memory | 0.13× | 0.10× |
| Lightpanda / Chromium, CPU | 0.27× | 0.26× |
| Lightpanda / Chromium, memory | 0.26× | 0.23× |

Read as:

- **Memory is where dasher is ahead**: four tenths of Lightpanda's peak
  on the median site and a tenth of Chromium's. It used less than
  Lightpanda on 322 of the 328 sites and less than Chromium on all of
  them.
- **CPU is level with Lightpanda**, 4% more in total: lower on 102
  sites, within 10% on 92, higher on 134. Both use about a quarter of
  Chromium's.
- **Wall time is mixed.** dasher finished sooner than Lightpanda on the
  median site, and later in total: Lightpanda stops waiting 5 s after it
  starts (90 sites reached that cap), where dasher keeps going until the
  page is quiet or a budget runs out. Ten dasher runs in 1,476 took 25 s
  or more.
- **Per-run spread is small**: over a site's three runs, CPU varied by
  4% of the median for dasher and Lightpanda (15% at the 90th
  percentile), 7% for Chromium.

### The same content

Loading a page is not the same work if the result differs, so the
comparison is repeated on sites where it does not. A site counts as
*same content* when dasher's and Lightpanda's visible text each share at
least 0.9 of their distinct words with Chromium's (Jaccard similarity of
the word sets).

| 237 same-content sites | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| CPU, total | 391 s | 368 s | 1459 s |
| CPU, median | 0.78 s | 0.76 s | 3.23 s |
| Peak memory, median | 34 MiB | 82 MiB | 364 MiB |

dasher used 1.06× Lightpanda's CPU on these (median site 1.04×) and
0.27× Chromium's.

dasher against Lightpanda alone, on sites where their texts are at
least 0.9 similar, their request counts within 25%, either spent more
than a CPU second and dasher ended on no budget (99 sites): dasher used
1.17× the CPU (median site 1.08×) and 0.56× the memory. This is the
closest the run comes to the two doing the same work, and there
dasher's interpreter (QuickJS) is behind Lightpanda's V8.

On light pages (Chromium under 2 CPU s, 85 sites) the median was 0.14 s
and 11 MiB for dasher, 0.15 s and 42 MiB for Lightpanda, 0.86 s and
263 MiB for Chromium.

### How much of the page came back

Against Chromium's text, on the sites both loaded:

| | dasher | Lightpanda |
| --- | ---: | ---: |
| Sites compared | 333 | 344 |
| Median similarity | 0.997 | 0.998 |
| At least 0.9 similar | 252 | 260 |
| Under 0.5 similar | 22 | 22 |
| Under half Chromium's text | 13 | 12 |

Chromium against itself, run to run, was at least 0.9 similar on 309 of
333 sites: the rest serve different text on each load.

The thirteen sites where dasher kept under half of Chromium's text,
with the page error behind it where there was one (see
[COMPATIBILITY.md](../../../COMPATIBILITY.md)):

- mongodb.com and seller.flipkart.com: a 2D context asked of a canvas
  that is not in the document, which dasher answers with `null`.
- google.com/maps: `XPathResult` is undefined.
- target.com: `WebAssembly` is undefined.
- azure.microsoft.com: each request took about 8 s to answer, and
  dasher fetches parser-blocking scripts one at a time.
- microsoft.com: the server answered dasher with a block page ("Your
  request has been blocked") under status 200, which the outcome rules
  here did not take for a bot wall; the `JSON.parse` error is that
  page's own JSON. From a home connection dasher gets the home page,
  0.97 similar to Chromium's text here.
- disneyplus.com and sooka.my: a `TypeError` in the page's own bundle,
  then React's hydration error. Lightpanda kept as little of
  disneyplus.com, and of word.cloud.microsoft.
- instagram.com: the server sends its error page to a navigation
  without `Sec-Fetch-*` headers, which dasher 0.1.0 did not send. It
  sends them since, and gets the page Chromium got.
- flashscore.ro: the consent banner, 17,400 characters of Chromium's
  text, was missing. Its script (OneTrust) took the result of
  `querySelectorAll` for an array, because `Array.isArray` said so in
  dasher 0.1.0, and failed in a promise. Fixed since: a load from a home
  connection is 0.996 similar to Chrome's.
- digikala.com: the page is built by scripts after load, so under the
  default visibility policy its shelves are below the fold and never
  load. `--visibility all` or `--scroll` returns them.
- yahoo.com: Yahoo sends a fuller page, about four times the text in
  the HTML, to a user agent that names `HeadlessChrome` (Chromium's
  here) or Lightpanda, and the short one to dasher, which names Chrome.
  Against Chrome with an ordinary user agent, from a home connection,
  what dasher lacks is the rest of the feed, which loads on coming into
  view (as on digikala.com), and the ad frames.

These were worked out after the run, from a home connection, on
2026-11-15; the numbers above are the run's and have not been redone.

### Limits

- dasher: 27 runs of 1,476 ended on the CPU budget (10 s) and 3 on the
  request budget; 4 reached the 30 s time limit. Ten sites ended on a
  budget on most runs (among them bloomberg.com, hubspot.com,
  huffpost.com, salesforce.com, twitch.tv). A budget is a success: the
  page is returned as it stood.
- Lightpanda: 266 runs stopped at its 5 s wait cap. With the cap at
  30 s it used 1.17× the CPU, kept over 10% more text on 19 sites, and
  was at least 0.9 similar to Chromium's text on 265 sites, against 260.
- Chromium: on 186 sites it was still making requests 5 s after `load`
  and was read as it stood.

### Starting up

Process start to exit on a page that costs nothing (one inline script,
served from the sandbox itself), ten sandboxes, ten runs each, medians:

| | dasher | Lightpanda | Chromium |
| --- | ---: | ---: | ---: |
| Print the version: wall | 10 ms | 10 ms | 31 ms |
| Load the page: wall | 67 ms | 41 ms | 324 ms |
| Load the page: CPU | 60 ms | 36 ms | 260 ms |
| Load the page: peak memory | 7.8 MiB | 22.9 MiB | 192 MiB |

Wall times are in steps of 10 ms, the runner's sampling period.
dasher's 60 ms is its own start-up and not the page's work (a static
page with no scripts costs it as much), and it is more than
Lightpanda's. Waiting for network idle on top of `load` cost dasher and
Lightpanda nothing and Chromium 500 ms.

### Cost

On Lambda the bill is wall time times configured memory. At the 3008 MB
these ran with, the mean page of the 328 comes to $122 per million for
dasher, $113 for Lightpanda and $261 for Chromium (us-east-1 arm64
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
  round shows no trend (dasher's five positions: 0.96, 0.94, 0.95, 0.88
  and 1.09 s, each over a different set of sites).
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
- **Hardware.** Lambda's arm64 fleet is mixed: 308 of these sandboxes
  reported Neoverse N1 cores, 181 Neoverse V1 and 3 Neoverse V2. Ratios
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
- **Sites behind bot protection.** A fifth of the list answered with a
  wall. Which tool gets through depends on its TLS and HTTP
  fingerprint and on the address it calls from more than on anything
  measured here.
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
docker build --platform linux/arm64 --provenance=false -t $REPO:latest image && docker push $REPO:latest
aws lambda create-function --function-name dasher-bench --package-type Image \
  --code ImageUri=$REPO:latest --architectures arm64 --memory-size 3008 --timeout 900 \
  --environment "Variables={BENCH_BUCKET=$BENCH_BUCKET}" \
  --role arn:aws:iam::$ACCOUNT:role/dasher-bench-role
# An asynchronous invocation that fails must not run again.
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
