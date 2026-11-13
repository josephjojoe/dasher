// Connects the DOM to the page loader. The loader is Rust (dasher-page);
// see src/loader.rs for what each hook is for.
(function (native, internals) {
  'use strict';

  const L = native.loader;

  // Inserted scripts that are not inline classic ones, for the loader to
  // prepare, and document.write() markup while the parser runs.
  internals.scriptConnected = (script, nonBlocking) => L.scriptInserted(script, nonBlocking);
  internals.documentWrite = (html) => L.write(internals.usv(html));
  // Links that load something. Module preloads fetch their module (see
  // src/modules/mod.rs). Stylesheets and other preloads are never fetched,
  // but get their load event as if they had loaded empty: scripts wait for
  // it (Vite's preload helper holds dynamic imports until their CSS loads).
  internals.linkConnected = (link) => {
    const rel = (link.getAttribute('rel') || '').toLowerCase().split(/[\t\n\f\r ]+/);
    if (rel.includes('modulepreload')) {
      native.modules.linkConnected(link);
    } else if (link.hasAttribute('href')) {
      internals.queueTask(() => internals.fire(link, 'load'));
    }
  };

  // Scripts' import() calls are renamed to call this global (see
  // src/modules/mod.rs), which fetches module graphs asynchronously.
  Object.defineProperty(globalThis, native.modules.importName, {
    value: native.modules.dynamicImport,
    writable: false, enumerable: false, configurable: false,
  });

  // Called by Rust.
  internals.loader = {
    setReadyState: internals.setReadyState,
    setCurrentScript: internals.setCurrentScript,
    setDocumentInfo: internals.setDocumentInfo,
    upgradeCustomElements: internals.upgradeCustomElements,
    bodyInserted: internals.bodyInserted,
    // A null target is the window.
    fire(target, type, bubbles, cancelable) {
      const init = { bubbles, cancelable };
      if (type === 'pageshow' || type === 'pagehide') {
        return internals.fire(target ?? globalThis, type, init, internals.PageTransitionEvent);
      }
      return internals.fire(target ?? globalThis, type, init);
    },
  };
})
