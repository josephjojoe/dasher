// The window: global aliases, Window as the global's prototype, location,
// history, navigator, screen and viewport, scrolling, postMessage,
// MessageChannel and requestIdleCallback.
// https://html.spec.whatwg.org/#the-window-object
//
// Nothing is laid out. Dimensions come from the configured viewport
// (native.env), and the document counts as tall enough to scroll anywhere.
(function (native, internals) {
  'use strict';

  const env = native.env;
  const usv = internals.usv;
  const domException = internals.domException;
  const eventClass = internals.eventClass;
  const fire = internals.fire;
  const queueTask = internals.queueTask;
  const structuredClone = internals.structuredClone;
  const structuredCloneWithTransfer = globalThis.structuredClone;
  const urlParse = internals.urlParse;
  const P = internals.URL_PARTS;
  const pageNow = native.now;
  const defineProperty = Object.defineProperty;
  const ReflectApply = Reflect.apply;
  const clearTimeoutRef = globalThis.clearTimeout;
  const KEY = Symbol('dasher');

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  function illegal(key) {
    if (key !== KEY) throw new TypeError('Illegal constructor');
  }

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to execute ' + what + ': ' + n + ' argument' + (n === 1 ? '' : 's') +
        ' required, but only ' + args.length + ' present.');
    }
  }

  // Defines a [Replaceable] attribute on the global: a getter whose setter
  // shadows it with a plain data property, as pages sometimes do.
  function replaceable(name, get) {
    defineProperty(globalThis, name, {
      get,
      set(value) {
        defineProperty(globalThis, name, { value, writable: true, enumerable: true, configurable: true });
      },
      enumerable: true,
      configurable: true,
    });
  }

  function readonly(name, get) {
    defineProperty(globalThis, name, { get, enumerable: true, configurable: true });
  }

  // Window. The global object takes Window.prototype as its prototype, so
  // `window instanceof Window` holds and EventTarget methods work on it.

  class Window extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
  }
  tag(Window, 'Window');
  Object.setPrototypeOf(globalThis, Window.prototype);
  // QuickJS tags its global object 'global'; browsers inherit 'Window'.
  delete globalThis[Symbol.toStringTag];

  defineProperty(globalThis, 'window', { get: () => globalThis, enumerable: true, configurable: false });
  for (const name of ['self', 'frames', 'parent', 'top']) replaceable(name, () => globalThis);

  internals.defineEventHandlers(Window.prototype, [
    'onabort', 'onafterprint', 'onbeforeprint', 'onbeforeunload', 'onblur', 'onerror', 'onfocus',
    'onhashchange', 'onlanguagechange', 'onload', 'onmessage', 'onmessageerror', 'onoffline',
    'ononline', 'onpagehide', 'onpageshow', 'onpopstate', 'onrejectionhandled', 'onresize',
    'onscroll', 'onstorage', 'onunhandledrejection', 'onunload',
  ]);

  const PopStateEvent = eventClass('PopStateEvent', { state: null, hasUAVisualTransition: false });
  const HashChangeEvent = eventClass('HashChangeEvent', { oldURL: '', newURL: '' });
  const PageTransitionEvent = eventClass('PageTransitionEvent', { persisted: false });
  const MessageEvent = eventClass('MessageEvent', {
    data: null,
    origin: '',
    lastEventId: '',
    source: null,
    ports: [],
  });

  // The document URL. Rust keeps a copy for the host (JsRuntime::url).

  let current = urlParse(env.url);

  function setCurrent(parts) {
    current = parts;
    native.setUrl(parts[P.HREF]);
  }

  function withoutFragment(href) {
    const i = href.indexOf('#');
    return i < 0 ? href : href.slice(0, i);
  }

  function documentURL() {
    return current[P.HREF];
  }

  function origin() {
    return current[P.ORIGIN];
  }

  // History. Every entry belongs to this document: going back past the
  // first entry or forward past the last does nothing.
  // https://html.spec.whatwg.org/#the-history-interface

  const MAX_HISTORY = 50;
  const entries = [{ url: current, state: null }];
  let index = 0;
  let scrollRestoration = 'auto';

  function addEntry(url, state, replace) {
    if (replace) {
      entries[index] = { url, state };
      return;
    }
    entries.splice(index + 1);
    entries.push({ url, state });
    if (entries.length > MAX_HISTORY) entries.shift();
    index = entries.length - 1;
  }

  function queueHashChange(oldURL, newURL) {
    queueTask(() => fire(globalThis, 'hashchange', { oldURL, newURL }, HashChangeEvent));
  }

  function traverse(delta) {
    queueTask(() => {
      const target = index + delta;
      if (target < 0 || target >= entries.length || target === index) return;
      const oldURL = current[P.HREF];
      index = target;
      const entry = entries[index];
      setCurrent(entry.url);
      fire(globalThis, 'popstate', { state: entry.state }, PopStateEvent);
      const newURL = entry.url[P.HREF];
      if (oldURL !== newURL && withoutFragment(oldURL) === withoutFragment(newURL)) {
        queueHashChange(oldURL, newURL);
      }
    });
  }

  function sameOriginForHistory(a, b) {
    return a[P.PROTOCOL] === b[P.PROTOCOL] && a[P.USERNAME] === b[P.USERNAME] &&
      a[P.PASSWORD] === b[P.PASSWORD] && a[P.HOST] === b[P.HOST];
  }

  function changeState(data, url, replace, what) {
    const state = structuredClone(data);
    let target = current;
    if (url !== undefined && url !== null) {
      target = urlParse(url, current[P.HREF]);
      if (target === undefined) {
        throw domException('SecurityError', 'Failed to execute ' + what + ": A history state object with URL '" +
          String(url) + "' cannot be created in a document with origin '" + origin() + "'.");
      }
      if (!sameOriginForHistory(target, current)) {
        throw domException('SecurityError', 'Failed to execute ' + what + ": A history state object with URL '" +
          target[P.HREF] + "' cannot be created in a document with origin '" + origin() +
          "' and URL '" + current[P.HREF] + "'.");
      }
    }
    addEntry(target, state, replace);
    setCurrent(target);
  }

  class History {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return entries.length;
    }
    get state() {
      return entries[index].state;
    }
    get scrollRestoration() {
      return scrollRestoration;
    }
    set scrollRestoration(value) {
      value = String(value);
      if (value === 'auto' || value === 'manual') scrollRestoration = value;
    }
    pushState(data, unused, url) {
      requireArgs(arguments, 2, "'pushState' on 'History'");
      changeState(data, url, false, "'pushState' on 'History'");
    }
    replaceState(data, unused, url) {
      requireArgs(arguments, 2, "'replaceState' on 'History'");
      changeState(data, url, true, "'replaceState' on 'History'");
    }
    go(delta = 0) {
      delta = Math.trunc(Number(delta)) || 0;
      if (delta === 0) {
        native.navigate(current[P.HREF], 'reload');
      } else {
        traverse(delta);
      }
    }
    back() {
      traverse(-1);
    }
    forward() {
      traverse(1);
    }
  }
  tag(History, 'History');
  const history = new History(KEY);

  // Location. Changing anything but the fragment asks the host to load a
  // new document (native.navigate); fragment changes stay in this one.
  // https://html.spec.whatwg.org/#the-location-interface

  function navigate(target, replace) {
    if (target[P.PROTOCOL] === 'javascript:') return;
    const href = target[P.HREF];
    if (href.includes('#') && withoutFragment(href) === withoutFragment(current[P.HREF])) {
      const oldURL = current[P.HREF];
      if (href === oldURL) return;
      addEntry(target, null, replace);
      setCurrent(target);
      queueHashChange(oldURL, href);
      return;
    }
    native.navigate(href, replace ? 'replace' : 'push');
  }

  function navigateTo(url, replace, what) {
    const target = urlParse(url, current[P.HREF]);
    if (target === undefined) {
      throw domException('SyntaxError', "Failed to execute '" + what + "' on 'Location': '" +
        String(url) + "' is not a valid URL.");
    }
    navigate(target, replace);
  }

  function setPart(part, value) {
    const target = internals.urlSet(current[P.HREF], part, value);
    if (part === 'hash' && target[P.HASH] === current[P.HASH]) return;
    navigate(target, false);
  }

  const ancestorOrigins = Object.freeze({
    length: 0,
    item: () => null,
    contains: () => false,
    [Symbol.toStringTag]: 'DOMStringList',
  });

  class Location {
    constructor(key) {
      illegal(key);
    }
    get href() { return current[P.HREF]; }
    set href(value) { navigateTo(usv(value), false, 'href'); }
    get origin() { return current[P.ORIGIN]; }
    get protocol() { return current[P.PROTOCOL]; }
    set protocol(value) { setPart('protocol', value); }
    get host() { return current[P.HOST]; }
    set host(value) { setPart('host', value); }
    get hostname() { return current[P.HOSTNAME]; }
    set hostname(value) { setPart('hostname', value); }
    get port() { return current[P.PORT]; }
    set port(value) { setPart('port', value); }
    get pathname() { return current[P.PATHNAME]; }
    set pathname(value) { setPart('pathname', value); }
    get search() { return current[P.SEARCH]; }
    set search(value) { setPart('search', value); }
    get hash() { return current[P.HASH]; }
    set hash(value) { setPart('hash', value); }
    get ancestorOrigins() { return ancestorOrigins; }
    assign(url) {
      requireArgs(arguments, 1, "'assign' on 'Location'");
      navigateTo(usv(url), false, 'assign');
    }
    replace(url) {
      requireArgs(arguments, 1, "'replace' on 'Location'");
      navigateTo(usv(url), true, 'replace');
    }
    reload() {
      native.navigate(current[P.HREF], 'reload');
    }
    toString() {
      return current[P.HREF];
    }
    valueOf() {
      return this;
    }
  }
  tag(Location, 'Location');
  const location = new Location(KEY);

  defineProperty(globalThis, 'location', {
    get: () => location,
    set(value) {
      location.href = value;
    },
    enumerable: true,
    configurable: false,
  });

  // Navigator. https://html.spec.whatwg.org/#the-navigator-object

  const userAgent = env.userAgent;
  const languages = Object.freeze(env.languages.slice());
  const chromeMatch = /\bChrome\/(\d+)/.exec(userAgent);
  const isChrome = chromeMatch !== null && !/\b(Edg|OPR)\//.test(userAgent);

  // Chrome's client hints, only when the user agent claims to be Chrome.
  // https://wicg.github.io/ua-client-hints/
  let userAgentData;
  if (chromeMatch !== null) {
    const major = chromeMatch[1];
    const fullVersion = (/\bChrome\/([\d.]+)/.exec(userAgent) || [])[1] || major;
    const brands = Object.freeze([
      Object.freeze({ brand: 'Not)A;Brand', version: '24' }),
      Object.freeze({ brand: 'Chromium', version: major }),
      Object.freeze({ brand: 'Google Chrome', version: major }),
    ]);
    const mobile = /\bMobile\b/.test(userAgent);
    const platform = /Windows/.test(userAgent) ? 'Windows'
      : /Android/.test(userAgent) ? 'Android'
        : /Mac OS X/.test(userAgent) ? 'macOS'
          : /CrOS/.test(userAgent) ? 'Chrome OS' : 'Linux';
    class NavigatorUAData {
      constructor(key) {
        illegal(key);
      }
      get brands() { return brands; }
      get mobile() { return mobile; }
      get platform() { return platform; }
      getHighEntropyValues(hints) {
        const values = { brands, mobile, platform };
        const all = {
          architecture: 'x86',
          bitness: '64',
          formFactors: [mobile ? 'Mobile' : 'Desktop'],
          fullVersionList: brands.map((b) => ({
            brand: b.brand,
            version: b.brand === 'Not)A;Brand' ? '24.0.0.0' : fullVersion,
          })),
          model: '',
          platformVersion: '',
          uaFullVersion: fullVersion,
          wow64: false,
        };
        for (const hint of hints === undefined ? [] : [...hints]) {
          if (Object.prototype.hasOwnProperty.call(all, hint)) values[hint] = all[hint];
        }
        return Promise.resolve(values);
      }
      toJSON() {
        return { brands, mobile, platform };
      }
    }
    tag(NavigatorUAData, 'NavigatorUAData');
    userAgentData = new NavigatorUAData(KEY);
  }

  class Navigator {
    constructor(key) {
      illegal(key);
    }
    get userAgent() { return userAgent; }
    get appVersion() { return userAgent.replace(/^Mozilla\//, ''); }
    get appName() { return 'Netscape'; }
    get appCodeName() { return 'Mozilla'; }
    get product() { return 'Gecko'; }
    get productSub() { return isChrome ? '20030107' : '20100101'; }
    get vendor() { return isChrome ? 'Google Inc.' : ''; }
    get vendorSub() { return ''; }
    get platform() { return env.platform; }
    get language() { return languages.length ? languages[0] : 'en-US'; }
    get languages() { return languages; }
    get onLine() { return true; }
    get cookieEnabled() { return true; }
    get hardwareConcurrency() { return env.hardwareConcurrency; }
    get deviceMemory() { return env.deviceMemory; }
    get maxTouchPoints() { return env.touch ? 5 : 0; }
    get webdriver() { return false; }
    get pdfViewerEnabled() { return true; }
    get doNotTrack() { return null; }
    javaEnabled() { return false; }
  }
  tag(Navigator, 'Navigator');
  if (userAgentData !== undefined) {
    defineProperty(Navigator.prototype, 'userAgentData', {
      get: () => userAgentData,
      enumerable: true,
      configurable: true,
    });
  }
  const navigator = new Navigator(KEY);

  // Screen and the viewport.

  class ScreenOrientation extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get type() {
      return env.screenWidth >= env.screenHeight ? 'landscape-primary' : 'portrait-primary';
    }
    get angle() {
      return 0;
    }
    lock() {
      return Promise.reject(domException('NotSupportedError', 'screen.orientation.lock() is not available on this device.'));
    }
    unlock() {}
  }
  tag(ScreenOrientation, 'ScreenOrientation');
  internals.defineEventHandlers(ScreenOrientation.prototype, ['onchange']);
  const orientation = new ScreenOrientation(KEY);

  class Screen extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get width() { return env.screenWidth; }
    get height() { return env.screenHeight; }
    get availWidth() { return env.screenWidth; }
    get availHeight() { return env.screenHeight; }
    get availLeft() { return 0; }
    get availTop() { return 0; }
    get colorDepth() { return 24; }
    get pixelDepth() { return 24; }
    get isExtended() { return false; }
    get orientation() { return orientation; }
  }
  tag(Screen, 'Screen');
  internals.defineEventHandlers(Screen.prototype, ['onchange']);
  const screen = new Screen(KEY);

  // Scrolling. Position is tracked so pages that read it after scrolling
  // see it move. In the default layout there is no upper bound, because the
  // document counts as tall enough; the DOM sets internals.scrollLimit() to
  // [maxX, maxY] when it lays elements out. A `scroll` event is queued once
  // per change, at internals.scrollTarget() (the window until the DOM
  // provides document), and internals.scrolled() is told of every change.
  let scrollX = 0;
  let scrollY = 0;
  let scrollEventQueued = false;
  internals.scrollTarget = () => globalThis;
  internals.scrollLimit = () => null;
  internals.scrolled = () => {};

  function scrollCoordinate(value, fallback) {
    if (value === undefined) return fallback;
    const n = Number(value);
    return Number.isFinite(n) ? n : 0;
  }

  function scrollWindow(x, y) {
    const limit = internals.scrollLimit();
    if (limit !== null) {
      x = Math.min(x, limit[0]);
      y = Math.min(y, limit[1]);
    }
    x = Math.max(0, x);
    y = Math.max(0, y);
    if (x === scrollX && y === scrollY) return;
    scrollX = x;
    scrollY = y;
    internals.scrolled();
    if (!scrollEventQueued) {
      scrollEventQueued = true;
      queueTask(() => {
        scrollEventQueued = false;
        fire(internals.scrollTarget(), 'scroll', { bubbles: internals.scrollTarget() !== globalThis });
      });
    }
  }

  function scrollTo(...args) {
    if (args.length === 1 && typeof args[0] === 'object' && args[0] !== null) {
      scrollWindow(scrollCoordinate(args[0].left, scrollX), scrollCoordinate(args[0].top, scrollY));
    } else {
      scrollWindow(scrollCoordinate(args[0], 0), scrollCoordinate(args[1], 0));
    }
  }

  function scrollBy(...args) {
    if (args.length === 1 && typeof args[0] === 'object' && args[0] !== null) {
      scrollWindow(scrollX + scrollCoordinate(args[0].left, 0), scrollY + scrollCoordinate(args[0].top, 0));
    } else {
      scrollWindow(scrollX + scrollCoordinate(args[0], 0), scrollY + scrollCoordinate(args[1], 0));
    }
  }

  internals.scrollPosition = () => [scrollX, scrollY];
  internals.scrollTo = (x, y) => scrollWindow(x, y);

  class VisualViewport extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get offsetLeft() { return 0; }
    get offsetTop() { return 0; }
    get pageLeft() { return scrollX; }
    get pageTop() { return scrollY; }
    get width() { return env.width; }
    get height() { return env.height; }
    get scale() { return 1; }
  }
  tag(VisualViewport, 'VisualViewport');
  internals.defineEventHandlers(VisualViewport.prototype, ['onresize', 'onscroll', 'onscrollend']);
  const visualViewport = new VisualViewport(KEY);

  class BarProp {
    constructor(key) {
      illegal(key);
    }
    get visible() {
      return true;
    }
  }
  tag(BarProp, 'BarProp');
  const barProp = new BarProp(KEY);

  // postMessage and message channels. Messages are structured-cloned and
  // delivered as tasks.
  // https://html.spec.whatwg.org/#web-messaging

  let isPort;

  function splitTransfer(transfer) {
    const ports = [];
    const buffers = [];
    for (const item of transfer === undefined || transfer === null ? [] : [...transfer]) {
      (isPort(item) ? ports : buffers).push(item);
    }
    return { ports, buffers };
  }

  function cloneMessage(message, transfer) {
    const { ports, buffers } = splitTransfer(transfer);
    return { data: structuredCloneWithTransfer(message, { transfer: buffers }), ports };
  }

  function postMessage(message, targetOrigin, transfer) {
    requireArgs(arguments, 1, "'postMessage' on 'Window'");
    let target = '/';
    let list = transfer;
    if (typeof targetOrigin === 'object' && targetOrigin !== null) {
      if (targetOrigin.targetOrigin !== undefined) target = String(targetOrigin.targetOrigin);
      list = targetOrigin.transfer;
    } else if (targetOrigin !== undefined) {
      target = String(targetOrigin);
    }
    if (target === '/') {
      target = origin();
    } else if (target !== '*') {
      const parsed = urlParse(target);
      if (parsed === undefined) {
        throw domException('SyntaxError', "Failed to execute 'postMessage' on 'Window': Invalid target origin '" +
          target + "' in a call to 'postMessage'.");
      }
      target = parsed[P.ORIGIN];
    }
    const { data, ports } = cloneMessage(message, list);
    const source = origin();
    queueTask(() => {
      if (target !== '*' && target !== origin()) return;
      fire(globalThis, 'message', { data, origin: source, source: globalThis, ports }, MessageEvent);
    });
  }

  class MessagePort extends EventTarget {
    #other = null;
    #pending = [];
    #started = false;
    #closed = false;

    constructor(key) {
      illegal(key);
      super();
    }

    static {
      isPort = (value) => typeof value === 'object' && value !== null && #other in value;
      internals.entangle = (a, b) => {
        a.#other = b;
        b.#other = a;
      };
      internals.deliverToPort = (port, message) => {
        if (port.#closed) return;
        if (port.#started) {
          fire(port, 'message', message, MessageEvent);
        } else {
          port.#pending.push(message);
        }
      };
    }

    postMessage(message, transfer) {
      requireArgs(arguments, 1, "'postMessage' on 'MessagePort'");
      const list = typeof transfer === 'object' && transfer !== null && !(Symbol.iterator in transfer)
        ? transfer.transfer : transfer;
      const { data, ports } = cloneMessage(message, list);
      const target = this.#other;
      if (target === null || this.#closed) return;
      queueTask(() => internals.deliverToPort(target, { data, ports }));
    }

    start() {
      if (this.#started) return;
      this.#started = true;
      const pending = this.#pending;
      this.#pending = [];
      for (const message of pending) queueTask(() => internals.deliverToPort(this, message));
    }

    close() {
      this.#closed = true;
      this.#pending = [];
      const other = this.#other;
      this.#other = null;
      if (other !== null && other.#other === this) other.#other = null;
    }
  }
  tag(MessagePort, 'MessagePort');
  internals.defineEventHandlers(MessagePort.prototype, ['onmessage', 'onmessageerror', 'onclose']);
  {
    // Setting onmessage starts the port, as in browsers.
    const desc = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
    defineProperty(MessagePort.prototype, 'onmessage', {
      get: desc.get,
      set(value) {
        ReflectApply(desc.set, this, [value]);
        this.start();
      },
      enumerable: true,
      configurable: true,
    });
  }

  class MessageChannel {
    #port1 = new MessagePort(KEY);
    #port2 = new MessagePort(KEY);
    constructor() {
      internals.entangle(this.#port1, this.#port2);
    }
    get port1() {
      return this.#port1;
    }
    get port2() {
      return this.#port2;
    }
  }
  tag(MessageChannel, 'MessageChannel');

  // requestIdleCallback. The page is treated as idle as soon as the
  // callback's turn comes, with the usual 50 ms budget. A callback asked
  // for from inside an idle callback waits for the next idle period, 50 ms
  // on, as in a browser with nothing else to do. Idle callbacks do not keep
  // the page busy: one that reschedules itself forever runs as page time
  // passes, but the page still counts as having nothing left to do.
  // https://w3c.github.io/requestidlecallback/

  const IDLE_PERIOD = 50;
  const setIdleTimer = internals.setIdleTimer;
  let inIdleCallback = false;

  class IdleDeadline {
    #start;
    #didTimeout;
    constructor(key, start, didTimeout) {
      illegal(key);
      this.#start = start;
      this.#didTimeout = didTimeout;
    }
    get didTimeout() {
      return this.#didTimeout;
    }
    timeRemaining() {
      return Math.max(0, 50 - (pageNow() - this.#start));
    }
  }
  tag(IdleDeadline, 'IdleDeadline');

  const idleTimers = new Map();
  let lastIdleId = 0;

  function requestIdleCallback(callback, options) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': parameter 1 is not of type 'Function'.");
    }
    void options;
    const id = ++lastIdleId;
    const timer = setIdleTimer(() => {
      idleTimers.delete(id);
      inIdleCallback = true;
      try {
        ReflectApply(callback, globalThis, [new IdleDeadline(KEY, pageNow(), false)]);
      } finally {
        inIdleCallback = false;
      }
    }, inIdleCallback ? IDLE_PERIOD : 0);
    idleTimers.set(id, timer);
    return id;
  }

  function cancelIdleCallback(id) {
    const timer = idleTimers.get(id);
    if (timer === undefined) return;
    idleTimers.delete(id);
    ReflectApply(clearTimeoutRef, globalThis, [timer]);
  }

  // Install everything on the global.

  internals.expose({
    Window, Location, History, Navigator, Screen, ScreenOrientation, VisualViewport, BarProp,
    PopStateEvent, HashChangeEvent, PageTransitionEvent, MessageEvent, MessagePort,
    MessageChannel, IdleDeadline, postMessage, requestIdleCallback, cancelIdleCallback,
    scrollTo, scrollBy, scroll: scrollTo,
  });

  readonly('history', () => history);
  readonly('navigator', () => navigator);
  replaceable('clientInformation', () => navigator);
  replaceable('screen', () => screen);
  replaceable('visualViewport', () => visualViewport);
  for (const name of ['locationbar', 'menubar', 'personalbar', 'scrollbars', 'statusbar', 'toolbar']) {
    replaceable(name, () => barProp);
  }
  replaceable('innerWidth', () => env.width);
  replaceable('innerHeight', () => env.height);
  replaceable('outerWidth', () => env.width);
  replaceable('outerHeight', () => env.height);
  replaceable('devicePixelRatio', () => env.devicePixelRatio);
  for (const name of ['screenX', 'screenY', 'screenLeft', 'screenTop']) replaceable(name, () => 0);
  replaceable('scrollX', () => scrollX);
  replaceable('pageXOffset', () => scrollX);
  replaceable('scrollY', () => scrollY);
  replaceable('pageYOffset', () => scrollY);
  replaceable('origin', origin);
  replaceable('length', () => 0);
  replaceable('opener', () => null);
  readonly('closed', () => false);
  readonly('frameElement', () => null);
  readonly('crossOriginIsolated', () => false);
  readonly('isSecureContext', () => {
    const protocol = current[P.PROTOCOL];
    const host = current[P.HOSTNAME];
    return protocol === 'https:' || protocol === 'wss:' || protocol === 'file:' ||
      host === 'localhost' || host.endsWith('.localhost') || host === '[::1]' ||
      /^127\.\d+\.\d+\.\d+$/.test(host);
  });
  defineProperty(globalThis, 'name', { value: '', writable: true, enumerable: true, configurable: true });
  defineProperty(globalThis, 'status', { value: '', writable: true, enumerable: true, configurable: true });

  Object.assign(internals, {
    documentURL,
    // The base URL for resolving relative URLs. The DOM bindings replace it
    // to honour <base href>.
    baseURL: documentURL,
    origin,
    isPort,
    MessageEvent,
    PageTransitionEvent,
  });
})
