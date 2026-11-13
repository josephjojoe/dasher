// IntersectionObserver over the synthetic layout (see LayoutMode in
// window.rs and the geometry section of dom.js).
// https://w3c.github.io/IntersectionObserver/
//
// Observations are updated when a target is observed, when the window or
// an element scrolls, and, through the layout hook, whenever the event loop
// has drained its tasks after the document changed. Entries are queued as
// the standard says (on the first update, then whenever isIntersecting or
// the crossed threshold changes) and delivered to the callback in a task.
//
// Under the 'all' visibility every rendered target intersects, with a
// ratio of 1, except one that content inserted before it has pushed further
// down the document: it counts as having left the viewport until the next
// scroll. That makes "load more" sentinels fire again each time the page is
// scrolled, as they do in a browser, where new content pushes them out of
// view. Under 'loaded' and 'viewport' targets intersect when their box
// overlaps the root's (the viewport, grown by rootMargin): under 'loaded'
// a target is pushed out by the elements inserted before it since the last
// reveal, and under 'viewport' by everything before it, less the scroll
// position.
(function (native, internals) {
  'use strict';

  const N = native.dom;
  const env = native.env;
  const geometry = internals.geometry;
  const domException = internals.domException;
  const queueTask = internals.queueTask;
  const defineProperty = Object.defineProperty;
  const ReflectApply = Reflect.apply;
  const freeze = Object.freeze;
  const DOMRectReadOnly = globalThis.DOMRectReadOnly;
  const Document = globalThis.Document;

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  // Observers that have targets.
  const active = new Set();
  // Bumped on every scroll; registrations note the one they last saw.
  let scrollEpoch = 0;
  let updateQueued = false;

  function queueUpdate() {
    if (updateQueued) return;
    updateQueued = true;
    queueTask(() => {
      updateQueued = false;
      update();
    });
  }

  internals.scrolled = () => {
    scrollEpoch++;
    if (active.size !== 0) queueUpdate();
  };

  function activate(observer) {
    if (active.size === 0) N.setLayoutHook(update);
    active.add(observer);
  }

  function deactivate(observer) {
    active.delete(observer);
    if (active.size === 0) N.setLayoutHook(null);
  }

  // rootMargin: one to four lengths in px or %, as in CSS margin.
  function parseMargin(text) {
    const parts = String(text).trim().split(/\s+/).filter((p) => p !== '');
    if (parts.length === 0) parts.push('0px');
    if (parts.length > 4) return null;
    const values = [];
    for (const part of parts) {
      const m = /^([-+]?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?)(px|%)$/i.exec(part);
      if (m === null) {
        if (/^[-+]?0*\.?0+$/.test(part)) {
          values.push([0, 'px']);
          continue;
        }
        return null;
      }
      values.push([Number(m[1]), m[2].toLowerCase()]);
    }
    while (values.length < 4) {
      // top right bottom left, as in the margin shorthand.
      values.push(values[[0, 0, 0, 1][values.length]]);
    }
    return values;
  }

  function marginText(margin) {
    return margin.map((m) => m[0] + m[1]).join(' ');
  }

  function resolve(margin, size) {
    return margin[1] === '%' ? (margin[0] * size) / 100 : margin[0];
  }

  function parseThresholds(value) {
    const list = value === undefined ? [0] : Array.isArray(value) ? value : [value];
    const out = [];
    for (const v of list) {
      const n = Number(v);
      if (!(n >= 0 && n <= 1)) {
        throw new RangeError("Failed to construct 'IntersectionObserver': Threshold values must be numbers between 0 and 1");
      }
      out.push(n);
    }
    if (out.length === 0) out.push(0);
    return out.sort((a, b) => a - b);
  }

  class IntersectionObserverEntry {
    #init;
    constructor(init) {
      if (init === null || typeof init !== 'object') {
        throw new TypeError("Failed to construct 'IntersectionObserverEntry': 1 argument required.");
      }
      const rect = (r) => (r === null || r === undefined ? null : DOMRectReadOnly.fromRect(r));
      this.#init = {
        time: Number(init.time) || 0,
        rootBounds: rect(init.rootBounds),
        boundingClientRect: rect(init.boundingClientRect) || new DOMRectReadOnly(),
        intersectionRect: rect(init.intersectionRect) || new DOMRectReadOnly(),
        isIntersecting: Boolean(init.isIntersecting),
        isVisible: Boolean(init.isVisible),
        intersectionRatio: Number(init.intersectionRatio) || 0,
        target: init.target,
      };
    }
    get time() { return this.#init.time; }
    get rootBounds() { return this.#init.rootBounds; }
    get boundingClientRect() { return this.#init.boundingClientRect; }
    get intersectionRect() { return this.#init.intersectionRect; }
    get isIntersecting() { return this.#init.isIntersecting; }
    get isVisible() { return this.#init.isVisible; }
    get intersectionRatio() { return this.#init.intersectionRatio; }
    get target() { return this.#init.target; }
  }
  tag(IntersectionObserverEntry, 'IntersectionObserverEntry');

  // What the update steps need of an observer, kept off the public object.
  const privates = new WeakMap();

  class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== 'function') {
        throw new TypeError("Failed to construct 'IntersectionObserver': The callback provided as parameter 1 is not a function.");
      }
      options = options === null || options === undefined ? {} : options;
      let root = options.root === undefined ? null : options.root;
      if (root !== null && !geometry.isElement(root) && !(root instanceof Document)) {
        throw new TypeError("Failed to construct 'IntersectionObserver': Failed to read the 'root' property from 'IntersectionObserverInit': The provided value is not of type '(Document or Element)'.");
      }
      const margin = parseMargin(options.rootMargin === undefined ? '0px' : options.rootMargin);
      if (margin === null) {
        throw domException('SyntaxError', "Failed to construct 'IntersectionObserver': rootMargin must be specified in pixels or percent.");
      }
      const scrollMargin = parseMargin(options.scrollMargin === undefined ? '0px' : options.scrollMargin);
      if (scrollMargin === null) {
        throw domException('SyntaxError', "Failed to construct 'IntersectionObserver': scrollMargin must be specified in pixels or percent.");
      }
      privates.set(this, {
        callback,
        root,
        margin,
        scrollMargin,
        thresholds: freeze(parseThresholds(options.threshold)),
        delay: Math.max(0, Number(options.delay) || 0),
        trackVisibility: Boolean(options.trackVisibility),
        // target -> its registration.
        targets: new Map(),
        records: [],
        notifyQueued: false,
      });
    }
    get root() { return privates.get(this).root; }
    get rootMargin() { return marginText(privates.get(this).margin); }
    get scrollMargin() { return marginText(privates.get(this).scrollMargin); }
    get thresholds() { return privates.get(this).thresholds; }
    get delay() { return privates.get(this).delay; }
    get trackVisibility() { return privates.get(this).trackVisibility; }
    observe(target) {
      if (!geometry.isElement(target)) {
        throw new TypeError("Failed to execute 'observe' on 'IntersectionObserver': parameter 1 is not of type 'Element'.");
      }
      const p = privates.get(this);
      if (p.targets.has(target)) return;
      p.targets.set(target, { thresholdIndex: -1, isIntersecting: false, line: -1, epoch: scrollEpoch });
      activate(this);
      queueUpdate();
    }
    unobserve(target) {
      const p = privates.get(this);
      p.targets.delete(target);
      if (p.targets.size === 0) deactivate(this);
    }
    disconnect() {
      privates.get(this).targets.clear();
      deactivate(this);
    }
    takeRecords() {
      const p = privates.get(this);
      const records = p.records;
      p.records = [];
      return records;
    }
  }
  tag(IntersectionObserver, 'IntersectionObserver');

  function rect(b) {
    return { x: b[0], y: b[1], width: b[2], height: b[3] };
  }

  // The root's box relative to the viewport, grown by the root margin.
  function rootBox(p) {
    let b;
    if (p.root === null || !geometry.isElement(p.root)) {
      b = [0, 0, env.width, env.height];
    } else {
      b = geometry.clientBox(p.root);
    }
    const m = p.margin;
    const top = resolve(m[0], b[3]);
    const right = resolve(m[1], b[2]);
    const bottom = resolve(m[2], b[3]);
    const left = resolve(m[3], b[2]);
    return [b[0] - left, b[1] - top, b[2] + left + right, b[3] + top + bottom];
  }

  // The update intersection observations steps, for every observer.
  function update() {
    if (active.size === 0) return;
    const observers = [...active];
    const targets = [];
    for (const observer of observers) {
      for (const target of privates.get(observer).targets.keys()) targets.push(target);
    }
    // Under 'loaded', the targets' boxes; under 'all', their lines, to
    // tell which were pushed down. One walk of the document for all.
    const linesOf = geometry.loaded ? N.loadedLines : N.layoutLines;
    let lines;
    try {
      lines = linesOf(targets);
    } catch (_) {
      // A target from a replaced document.
      lines = targets.flatMap((t) => {
        try {
          return linesOf([t]);
        } catch (_) {
          return [-1, 0];
        }
      });
    }
    const time = native.now();
    let i = 0;
    for (const observer of observers) {
      const p = privates.get(observer);
      const root = rootBox(p);
      const rootElement = p.root !== null && geometry.isElement(p.root) ? p.root : null;
      const rootRendered = rootElement === null || geometry.rendered(rootElement);
      for (const [target, reg] of p.targets) {
        const line = lines[2 * i];
        i++;
        let visible = rootRendered && geometry.rendered(target);
        if (visible && rootElement !== null) {
          visible = rootElement !== target && geometry.contains(rootElement, target);
        }
        let box = [0, 0, 0, 0];
        if (visible) box = geometry.loaded ? geometry.loadedBox(lines, 2 * (i - 1)) : geometry.clientBox(target);
        let isIntersecting = false;
        let ratio = 0;
        let hit = [0, 0, 0, 0];
        if (visible && (geometry.scrolled || geometry.loaded)) {
          const x0 = Math.max(box[0], root[0]);
          const y0 = Math.max(box[1], root[1]);
          const x1 = Math.min(box[0] + box[2], root[0] + root[2]);
          const y1 = Math.min(box[1] + box[3], root[1] + root[3]);
          if (x1 >= x0 && y1 >= y0) {
            isIntersecting = true;
            hit = [x0, y0, x1 - x0, y1 - y0];
            const area = box[2] * box[3];
            ratio = area === 0 ? 1 : (hit[2] * hit[3]) / area;
          }
        } else if (visible) {
          // Pushed down since the last scroll: out of view until the next.
          if (reg.epoch !== scrollEpoch || reg.line < 0) {
            reg.epoch = scrollEpoch;
            reg.line = line;
          }
          isIntersecting = !(line > reg.line);
          if (isIntersecting) {
            hit = box;
            ratio = 1;
          }
        }
        let thresholdIndex = p.thresholds.findIndex((t) => t > ratio);
        if (thresholdIndex < 0) thresholdIndex = p.thresholds.length;
        if (thresholdIndex === reg.thresholdIndex && isIntersecting === reg.isIntersecting) continue;
        reg.thresholdIndex = thresholdIndex;
        reg.isIntersecting = isIntersecting;
        p.records.push(new IntersectionObserverEntry({
          time,
          rootBounds: rect(root),
          boundingClientRect: rect(box),
          intersectionRect: rect(hit),
          isIntersecting,
          isVisible: p.trackVisibility && isIntersecting,
          intersectionRatio: ratio,
          target,
        }));
        notify(observer, p);
      }
    }
  }

  function notify(observer, p) {
    if (p.notifyQueued) return;
    p.notifyQueued = true;
    queueTask(() => {
      p.notifyQueued = false;
      const records = observer.takeRecords();
      if (records.length !== 0) ReflectApply(p.callback, observer, [records, observer]);
    });
  }

  internals.expose({ IntersectionObserver, IntersectionObserverEntry });
})
