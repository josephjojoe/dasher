# Compatibility fixtures

Small apps built with the frameworks and libraries scrapers meet, from
their real production builds. [`../compat.rs`](../compat.rs) loads each
from the fixture server, settles it as a scrape would, checks the
content in the final HTML and drives the interactions a scrape may
need. A test fails on any uncaught error or console error.

| App | Path | What it exercises |
| --- | --- | --- |
| `react-spa` | `/compat/react-spa/` | React 19 rendering an empty `#root` client-side: `fetch` in effects, StrictMode, Suspense with `lazy`, portals, `useLayoutEffect` measuring, `<title>` hoisting, delegated events, a controlled input |
| `next` | `/compat/next/` | A Next.js 16 static export. App Router: server HTML hydrated from the inline RSC payload, a client component fetching after hydration, `<Link>` navigation through RSC fetches, `history.back()`. Pages Router (`/compat/next/legacy`): `getStaticProps` and `__NEXT_DATA__` |
| `vue` | `/compat/vue/` | Vue 3's global build compiling an in-DOM template at runtime, components with string templates, `v-model`, axios (XHR) |
| `vite` | `/compat/vite/` | A Vite 6 production build of a Preact app: module scripts, `modulepreload`, a vendor chunk, a dynamically imported route chunk with its own CSS |
| `jquery` | `/compat/jquery/` | jQuery 3.7.1: `$.getJSON` with a serialized form, `.load()` of a page fragment, JSONP, a POST with `$.ajax`, delegated events |
| `lit` | `/compat/lit/` | Lit 3 web components rendering into shadow roots: reactive properties, `static styles`, `repeat`, slots, reflected attributes, a composed event from a nested shadow tree, data fetched after load; a plain custom element rewriting its own text; a server-rendered declarative shadow root that its component hydrates |
| `router` | `/compat/router/` | React Router 7's data router with history routing: deep links answered with the app's one document, loaders, `<Link>` pushes, `popstate`, search params, a not-found route |

The built output in [`../fixtures/compat`](../fixtures/compat) is
committed, so the tests need nothing but Cargo. To change an app or
update a library, edit the sources under `apps/` and rebuild with
Node 20 or later:

```sh
cd crates/page/tests/compat
npm ci
npm run build   # rewrites ../fixtures/compat
```

The exact versions are in `package-lock.json`. The vendored libraries
keep their licence headers; all are MIT-licensed.
