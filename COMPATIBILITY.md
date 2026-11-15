# Web compatibility

dasher runs real sites' JavaScript well enough for scraping: the
compatibility suite (`crates/page/tests/compat.rs`, with the apps described in
`crates/page/tests/compat/README.md`) loads production builds of React,
Next.js (App and Pages Router), Vue, Vite, jQuery, React Router and Lit and checks
the content they render.

Some APIs are left out on purpose (see the project README's non-goals), some
are harmless stand-ins (every one is listed at the top of
`crates/js/src/stubs.js`), and some are simply not there yet. This is the
short list of known gaps a page may hit.

Web components work: custom elements are defined, constructed, upgraded
and get their lifecycle callbacks, and shadow roots (including declarative
ones from the markup) hold their content. Shadow trees are part of the
scraped output: `page.html()` / `content()` writes each one as declarative
shadow DOM (`<template shadowrootmode="open">` first in its host), and the
Rust selector helpers look into shadow trees. Scripts keep the browser's
view, where `querySelector` and `innerHTML` stop at shadow roots.

## Not supported

| API | What pages see |
| --- | --- |
| `slotchange` events | Never fire; `assignedNodes()` and `assignedSlot` are always current |
| `adoptedCallback`, scoped custom element registries | `adoptedCallback` never runs (there is one document); `new CustomElementRegistry()` throws |
| Form-associated custom elements | `attachInternals()` works, but every control counts as valid and its value is never submitted |
| `:defined`, `:host`, `::part()`, `::slotted()` in selectors | `:defined` matches every element; the shadow-scoped ones never match |
| `XMLSerializer` | Serializes as `outerHTML` does, with `xmlns` declared on the outermost element; empty XML elements are not self-closed (`<i></i>`, not `<i/>`) |
| DTD entities and processing instructions in XML documents | Entities a DTD declares are an error (as in browsers without the DTD); processing instructions are dropped |
| `document.evaluate` / XPath | Undefined, as are `XPathResult` and `XPathEvaluator` |
| Live ranges, `StaticRange` | A `Range` works (boundary points, contents, `createContextualFragment`) but does not follow later changes to the tree, and its rectangles are those of its closest element; `StaticRange` and `AbstractRange` are undefined |
| Choosing files | The `files` of an `<input type=file>` is a `FileList` that is always empty |
| `FileReader` | Undefined; use `blob.text()` / `arrayBuffer()` |
| Streaming fetch | Request bodies cannot be streams; response bodies arrive whole, then stream as one chunk |
| BYOB stream readers | `getReader({ mode: 'byob' })` throws; byte streams act as default streams |
| `crypto.subtle` beyond `digest()` | Every other method rejects with `NotSupportedError` |
| `Intl.Segmenter`, `Intl.DisplayNames`; locales other than en-US | Both are undefined; every locale formats as en-US. `Intl.Locale` has no `getWeekInfo()`, `getTextInfo()` or the other `get…()` methods, and `maximize()` and `minimize()` leave all but about fifty languages as they are |
| Performance timeline beyond navigation and user timing | `getEntries…()` list one `navigation` entry and the page's marks and measures; there are no `resource`, `paint` or other entries, and `PerformanceObserver` never calls back. `performance.timing` and the navigation entry time the document's stages (`domInteractive` to `loadEventEnd`), but every network stage reports the navigation start |
| Very deep recursion | A `RangeError` after about 6,400 nested JS calls when the page runs on a thread with 8 MiB of stack (the CLI's and the service's), fewer on smaller threads; Chrome allows about 10,000 |
| iframes | Not loaded |
| Workers | `Worker` and `SharedWorker` can be constructed, but the worker's script never loads: an `error` event follows and messages go nowhere |
| Service workers, `indexedDB`, `caches`, WebAssembly | Absent, so feature detection takes its fallback path. Sites that refuse to run without them show their fallback: duolingo.com redirects to its "browser not supported" page for lack of WebAssembly |
| Canvas drawing | `getContext('2d')` on a `<canvas>` in the document, or on an `OffscreenCanvas`, returns a context that keeps its state and draws nothing; `getImageData` reads transparent pixels and `measureText` estimates widths from the font size. A canvas never inserted gets `null`, as WebGL contexts always do: fingerprinting and bot-detection scripts probe with those and spend seconds of CPU when the probe works. Lottie probes that way too, so a page that needs Lottie to start (mongodb.com) renders nothing |
| WebSocket, EventSource | Present but always fail to connect |
