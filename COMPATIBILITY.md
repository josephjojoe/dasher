# Web compatibility

dasher runs a page's JavaScript well enough to scrape it. It is not a
browser to test a site in: nothing is laid out or painted, and the APIs a
scraper never needs are stand-ins or absent. This file says what a page
sees.

Everything below was checked against the 0.1.0 release build. The
framework suite in [`crates/page/tests/compat.rs`](crates/page/tests/compat.rs)
(apps in [`crates/page/tests/compat`](crates/page/tests/compat/README.md))
loads production builds of React 19, Next.js 16 (App and Pages Router),
Vue 3, a Vite 6 bundle of Preact, jQuery 3.7, React Router 7 and Lit 3,
and checks what they render and that they log no errors.

## What is there

- **The language.** QuickJS-NG 0.11: ES2023 and most of what came after
  (iterator helpers, the new `Set` methods, `Array.fromAsync`,
  `Object.groupBy`, `Promise.withResolvers`, `Promise.try`,
  `RegExp.escape`, `Float16Array`, `Error.isError`, `WeakRef`), classic
  and module scripts, import maps, dynamic `import()` and top-level
  `await`. Not `using` declarations, decorators or `Temporal`.
- **The DOM.** Documents, elements, text, attributes, fragments, ranges,
  tree walkers, `MutationObserver`, events with capture and bubbling,
  inline event handlers, forms and their controls' state, `DOMParser`,
  `innerHTML` and friends, `document.write`, templates, selectors.
- **Web components.** Custom elements are defined, constructed and
  upgraded and get their lifecycle callbacks; shadow roots, declarative
  ones included, hold their content. Shadow trees are part of what dasher
  returns: the HTML carries each as declarative shadow DOM
  (`<template shadowrootmode="open">` first in its host), and selectors
  given to dasher (the CLI's `--wait-for`, a job's `extract`, the Rust
  selector helpers) look into shadow trees, open or closed. Scripts keep
  a browser's view, where `querySelector` and `innerHTML` stop at a
  shadow root.
- **The network.** `fetch` with `Headers`, `Request`, `Response`, `Blob`,
  `File`, `FormData` and `AbortController`; `XMLHttpRequest`, the
  synchronous kind included; cookies (`document.cookie` and the jar),
  redirects, gzip, brotli and zstd, HTTP/1.1 and HTTP/2, an HTTP proxy.
  The same-origin policy is not enforced: no preflight is sent, and a
  script can read any response it fetches.
- **The window.** Timers, `requestAnimationFrame`, `requestIdleCallback`,
  `queueMicrotask`, `postMessage` and `MessageChannel`, `structuredClone`,
  `history` and `location` (script navigations and short meta refreshes
  are followed), `localStorage` and `sessionStorage`, `matchMedia`,
  `URL` and `URLSearchParams`, `TextEncoder` and `TextDecoder`, streams,
  `crypto.getRandomValues`, `randomUUID` and `subtle.digest`, the
  console, `performance.now`, marks and measures.
- **Intl.** `Collator`, `DateTimeFormat` (with time zones), `DisplayNames`,
  `ListFormat`, `Locale`, `NumberFormat`, `PluralRules` and
  `RelativeTimeFormat`, formatting as en-US whatever locale is asked for.

## Nothing is rendered

Only documents, scripts and what scripts request (`fetch`, XHR, modules)
go over the network. Images, stylesheets, fonts and media are never
downloaded.

| What a page asks | What it gets |
| --- | --- |
| Is this stylesheet loaded? | `<link rel=stylesheet>` gets its `load` event as if it had loaded empty. No rule of any stylesheet, `<style>` elements included, is parsed or applied: `sheet.cssRules` is empty. |
| `getComputedStyle()` | The element's inline style and the defaults of its tag and attributes (`hidden`, for one). Nothing from stylesheets, so a class that hides an element does not hide it here. |
| `getBoundingClientRect()`, `offsetHeight`, scroll positions | Synthetic boxes in a 1920×1080 window (`--viewport`). Elements with `display: none` inline, or `hidden`, are empty. |
| `IntersectionObserver`, `loading=lazy` | Decided by the visibility policy (`--visibility`): by default the page as it stood at the load event is in view, and what it adds later is below the fold until dasher scrolls. A page that builds itself after load therefore keeps back what it loads on coming into view: digikala.com returns its header, menus and footer, and yahoo.com one story of its feed, until `--scroll` or `--visibility all`. |
| `matchMedia()` | Evaluated against the viewport: a desktop screen, light colour scheme, no reduced motion, not print. |
| Has this image loaded? | No. An `<img>` never fires `load` or `error`; `complete` is `true` and `naturalWidth` is `0`. |
| `ResizeObserver` | Reports each observed element once, with its synthetic size. |
| `document.fonts`, animations, transitions | Fonts count as loaded; `element.animate()` returns an animation that has finished; no transition or animation event fires. |

## Not supported

| API | What pages see |
| --- | --- |
| WebAssembly | `WebAssembly` is undefined. Sites that refuse to run without it show their fallback: duolingo.com answers with its "browser not supported" page, and target.com renders a fraction of its page. |
| iframes | Not loaded: `contentWindow` and `contentDocument` are `null`. |
| Workers | `Worker` and `SharedWorker` can be constructed, but the script never loads: an `error` event follows and messages go nowhere. |
| Service workers, `indexedDB`, `caches`, `cookieStore` | Undefined, so feature detection takes its fallback path. |
| WebSocket, EventSource | Present, and always fail to connect: an `error` event, then closed. |
| Canvas | `getContext('2d')` on a `<canvas>` in the document, or on an `OffscreenCanvas`, gives a context that keeps its state and draws nothing: `getImageData` reads transparent pixels, `measureText` estimates from the font size and `toDataURL()` is `data:,`. A canvas that was never inserted gets `null`, as every WebGL context does. That is on purpose: fingerprinting and bot-detection scripts probe that way and spend seconds of CPU when the probe works. Pages that draw on a detached canvas while rendering throw instead: mongodb.com and seller.flipkart.com come back nearly empty. |
| Elements as globals | An element is not reachable through its `id` or `name` as a property of `window`, nor a form or image through its `name` as a property of `document`: with `<input id="menu">` in the page, `window.menu` is `undefined` and a bare `menu` is a `ReferenceError`. `document.getElementById()` and `document.forms.name` work. racingnews365.nl's menu script throws on it. |
| `window.event` | Not there: `window.event` is `undefined` while a listener runs, and a bare `event` in a listener that does not take the event as its argument is a `ReferenceError`. Handlers written as attributes (`onclick="go(event)"`) get their `event`. |
| Form submission | `form.submit()` and a submit button's click fire `submit` and go no further: no request is made and the page does not navigate. `new FormData(form)` holds the form's entries, with an empty file for each file input. |
| Choosing files | An `<input type=file>` has an empty `FileList`. |
| `FileReader` | Undefined; `blob.text()` and `blob.arrayBuffer()` work. |
| `crypto.subtle` beyond `digest()` | Every other method rejects with `NotSupportedError`. |
| Streaming fetch | A request body cannot be a stream (`TypeError`). A response body arrives whole and then streams as one chunk. `getReader({ mode: 'byob' })` throws. `CompressionStream` and `DecompressionStream` are undefined. |
| `document.evaluate` / XPath | Undefined, as are `XPathResult` and `XPathEvaluator`. google.com/maps stops on it. |
| Live ranges, `StaticRange` | A `Range` works (boundary points, contents, `createContextualFragment`) but does not follow later changes to the tree, and its rectangle is its closest element's. `StaticRange` and `AbstractRange` are undefined. |
| `slotchange` | Never fires. `assignedNodes()` and `assignedSlot` are always current. |
| `adoptedCallback`, scoped registries | `adoptedCallback` never runs; `new CustomElementRegistry()` throws. |
| Form-associated custom elements | `attachInternals()` works, but every control counts as valid and a custom element's value is not part of its form's data. |
| `:defined`, `:host`, `::part()`, `::slotted()` | `:defined` matches every element; the shadow-scoped ones match nothing. |
| `XMLSerializer` | Serializes as `outerHTML` does, with `xmlns` on the outermost element; an empty XML element is `<i></i>`, not `<i/>`. |
| XML documents | An entity declared in a DTD is a parse error; processing instructions are dropped. |
| Locales | Every locale formats as en-US, and `Intl.DisplayNames` gives English names. `Intl.Segmenter` is undefined. `Intl.Locale` has no `getWeekInfo()` or the other `get…()` methods, and `maximize()` knows about fifty languages. `navigator.language` is `en-US` and the time zone `America/New_York`. |
| RegExp `v` flag | The flag is accepted, but under it `\p{…}` matches the letters literally and set operations (`[\p{L}--[a-z]]`) are a `SyntaxError`. Property escapes work under `u`. This is QuickJS-NG's behaviour. |
| Performance timeline | `getEntries…()` hold one `navigation` entry and the page's marks and measures; there are no `resource` or `paint` entries, and a `PerformanceObserver` is never called. `performance.timing` times the document's stages (`domInteractive` to `loadEventEnd`); every network stage reports the navigation start. |
| `iframe.sandbox.supports()` | Throws a `TypeError`, where Chrome answers. On purpose: an answer lets Google Publisher Tag render ads into SafeFrames, at seconds of CPU and tens of MiB a page for no content. |
| Deep recursion | A `RangeError` after about 8,000 nested calls of a small function on the 8 MiB stacks the CLI and the service run pages on; Chrome allows about 10,000. |
| `navigation`, `scheduler`, `trustedTypes`, `URLPattern`, `DOMMatrix`, `FontFace`, `createImageBitmap`, speech synthesis, WebRTC, payment requests | Undefined. |

The stand-ins a page can call without effect (`alert`, `sendBeacon`,
`navigator.clipboard`, `Notification`, `BroadcastChannel` and the rest)
are all in one file, listed at the top of
[`crates/js/src/stubs.js`](crates/js/src/stubs.js).

## What dasher tells sites about itself

`navigator.userAgent` and the `User-Agent` header are desktop Chrome's on
Linux (`--user-agent` changes both), `navigator.userAgentData` matches,
and `navigator.webdriver` is `false`. Requests carry the fetch metadata
headers a browser sends (`Sec-Fetch-Dest`, `Sec-Fetch-Mode`,
`Sec-Fetch-Site`, `Sec-Fetch-User`), which servers read to tell a
navigation from a script's request: instagram.com serves its error page
to a navigation without them. Whether two hosts are the same site is
judged without the public suffix list. dasher does nothing else to look
like a person: it sends no client-hint headers, its TLS and HTTP/2
handshakes are not Chrome's, and it solves no challenges. Sites behind
bot protection (Cloudflare, Akamai, DataDome, PerimeterX) often answer
it with a challenge page, as they answer any client that is not a full
browser; on 492 popular sites fetched from AWS that was about one site
in five, for Chromium's headless shell as much as for dasher (see
[crates/bench/lambda](crates/bench/lambda/README.md)). The refusal is
not always a challenge status: microsoft.com answered dasher from AWS
with a block page under status 200, and with its home page from a home
connection.

Some sites serve more to a client that says it is automated. yahoo.com
sends a page with about four times the text in its HTML when the
`User-Agent` has `HeadlessChrome` in it, as Chromium's headless shell's
does, or is Lightpanda's; dasher, saying it is Chrome, gets the page a
person's browser gets, most of which arrives later from scripts.

dasher does not read `robots.txt` and does not limit its own request
rate. Which pages to fetch, and how often, is for the program that
drives it to decide.

## Network gaps

- **TLS** is rustls: TLS 1.2 and 1.3 with ECDHE key exchange. A server
  that offers only finite-field Diffie-Hellman (`DHE-RSA`) suites or
  nothing newer than TLS 1.1 cannot be connected to.
- **HTTP/3** is not spoken; servers are reached over HTTP/2 or HTTP/1.1.
- **No preload scanner.** Scripts that block the parser are fetched one
  at a time, each when the parser reaches it, where a browser fetches
  ahead. On a server that is slow to answer every request this adds up:
  azure.microsoft.com took about 8 s to answer each request when
  measured, and ran to dasher's time limit. `async`, `defer` and module scripts are
  fetched in parallel.
- **Proxies** are HTTP proxies (`CONNECT` for `https`), not SOCKS.
