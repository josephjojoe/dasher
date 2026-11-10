// matchMedia and MediaQueryList. Queries are evaluated in Rust against the
// configured viewport; nothing ever resizes, so `change` never fires.
// https://drafts.csswg.org/cssom-view/#the-mediaquerylist-interface
(function (native, internals) {
  'use strict';

  const usv = internals.usv;
  const KEY = Symbol('MediaQueryList');

  class MediaQueryList extends EventTarget {
    #media;
    #matches;
    constructor(key, media, matches) {
      super();
      if (key !== KEY) throw new TypeError('Illegal constructor');
      this.#media = media;
      this.#matches = matches;
    }
    get media() {
      return this.#media;
    }
    get matches() {
      return this.#matches;
    }
    // The legacy listener methods are aliases for `change` listeners.
    addListener(callback) {
      if (callback !== null && callback !== undefined) this.addEventListener('change', callback);
    }
    removeListener(callback) {
      if (callback !== null && callback !== undefined) this.removeEventListener('change', callback);
    }
  }
  Object.defineProperty(MediaQueryList.prototype, Symbol.toStringTag, {
    value: 'MediaQueryList',
    configurable: true,
  });
  internals.defineEventHandlers(MediaQueryList.prototype, ['onchange']);

  const MediaQueryListEvent = internals.eventClass('MediaQueryListEvent', { media: '', matches: false });

  function matchMedia(query) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'matchMedia' on 'Window': 1 argument required, but only 0 present.");
    }
    const text = usv(query);
    const matches = native.matchMedia(text);
    const media = text.trim().replace(/\s+/g, ' ').toLowerCase();
    return new MediaQueryList(KEY, media, matches);
  }

  internals.expose({ MediaQueryList, MediaQueryListEvent, matchMedia });
})
