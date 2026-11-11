// Connects the DOM to the page loader. The loader is Rust (dasher-page);
// see src/loader.rs for what each hook is for.
(function (native, internals) {
  'use strict';

  const L = native.loader;

  // Inserted scripts that are not inline classic ones, for the loader to
  // prepare, and document.write() markup while the parser runs.
  internals.scriptConnected = (script, nonBlocking) => L.scriptInserted(script, nonBlocking);
  internals.documentWrite = (html) => L.write(internals.usv(html));

  // Called by Rust.
  internals.loader = {
    setReadyState: internals.setReadyState,
    setCurrentScript: internals.setCurrentScript,
    setDocumentInfo: internals.setDocumentInfo,
    // A null target is the window.
    fire(target, type, bubbles, cancelable) {
      const init = { bubbles, cancelable };
      if (type === 'pageshow' || type === 'pagehide') {
        return internals.fire(target ?? globalThis, type, init, internals.PageTransitionEvent);
      }
      return internals.fire(target ?? globalThis, type, init);
    },
    document: internals.document,
  };
})
