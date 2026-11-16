# Security

## Reporting a vulnerability

Please report vulnerabilities privately, through GitHub's
[private vulnerability reporting](https://github.com/josephjojoe/dasher/security/advisories/new),
and not in a public issue. Say what you found, how to reproduce it and
what it lets an attacker do.

Fixes go into the latest release; there are no maintained older
branches.

## What counts

dasher runs JavaScript from pages it is told to load, so the page is
the attacker. These are vulnerabilities:

- a page reading or writing files, starting processes, or running native
  code;
- a page crashing the process or the service (as opposed to failing its
  own load), or getting past its memory, DOM, CPU or time limits;
- a page in `dasher serve` or `dasher lambda` reaching a loopback,
  link-local or private address, a cloud metadata endpoint or the Lambda
  Runtime API, by any route: redirects, DNS rebinding, odd URL forms;
- one job reading another job's cookies, storage or content;
- getting past `DASHER_AUTH_TOKEN`.

These are not:

- a caller of the service making it fetch a public URL. That is what it
  is for; keep it behind authentication (see
  [docs/deploy.md](docs/deploy.md#security));
- a page reaching private addresses through the CLI or the library with
  default settings. They make no such check, since the person running
  them chose the URL; `BrowserBuilder::block_private_addresses(true)`
  turns the check on;
- a proxy resolving a public name to a private address. The proxy
  connects out of dasher's sight; use one that refuses private
  destinations;
- sites detecting dasher as a bot.

## Dependencies

`cargo deny check` runs in CI against the RustSec advisory database,
with the licence and source policy in [`deny.toml`](deny.toml).
