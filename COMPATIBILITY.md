# Web compatibility

dasher runs real sites' JavaScript well enough for scraping: the
compatibility suite (`crates/page/tests/compat.rs`, with the apps described in
`crates/page/tests/compat/README.md`) loads production builds of React,
Next.js (App and Pages Router), Vue, Vite, jQuery and React Router and checks
the content they render.

Some APIs are left out on purpose (see the project README's non-goals), some
are harmless stand-ins (every one is listed at the top of
`crates/js/src/stubs.js`), and some are simply not there yet. This is the
short list of known gaps a page may hit.

## Not supported

| API | What pages see |
| --- | --- |
| Custom elements (`customElements.define`, upgrades, lifecycle callbacks) | `customElements` is undefined; custom tags stay plain `HTMLElement`s |
| Slots and event retargeting in shadow DOM | `attachShadow` works, but slotted nodes are never assigned and events crossing a shadow boundary keep their inner target |
| Declarative shadow DOM (`<template shadowrootmode>`) | Stays an ordinary `<template>` |
| Shadow trees in the output | `page.html()` does not include shadow root contents |
| `DOMParser`, `XMLSerializer`, XML documents, `responseXML` | `DOMParser` is undefined; `document.implementation.createHTMLDocument()` is the only way to make a second document |
| `document.evaluate` / XPath | Undefined |
| `Range` constructor | Undefined (`document.createRange()` exists) |
| `FileReader` | Undefined; use `blob.text()` / `arrayBuffer()` |
| Streaming fetch | Request bodies cannot be streams; response bodies arrive whole, then stream as one chunk |
| BYOB stream readers | `getReader({ mode: 'byob' })` throws; byte streams act as default streams |
| `crypto.subtle` beyond `digest()` | Every other method rejects with `NotSupportedError` |
| `Intl.Segmenter`; locales other than en-US | `Intl.Segmenter` is undefined; every locale formats as en-US |
| Performance timeline (`performance.timing`, entries from `mark`/`measure`) | `performance.timing` is undefined; `mark` and `measure` record nothing; `performance.now()` works |
| Inline event handler attributes (`onclick="..."`, `<body onload>`) | Not run from markup |
| `<meta http-equiv="refresh">` | Not followed |
| iframes | Not loaded |
| Workers, service workers, `indexedDB`, `caches`, WebAssembly | Absent, so feature detection takes its fallback path |
| WebSocket, EventSource | Present but always fail to connect |
