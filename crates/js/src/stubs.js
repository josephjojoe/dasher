// Harmless stand-ins for APIs pages feature-detect or call in passing but
// that a scraper never needs to work. EVERY STUB LIVES IN THIS FILE, so the
// list below is the complete set of gaps that are papered over.
//
// Stubbed (present, but does not do the real thing):
//
//   alert, confirm, prompt, print   alert() is logged to the console at info
//                                   level; confirm() returns false; prompt()
//                                   returns null; print() does nothing.
//   open, close, stop, focus, blur, moveTo, moveBy, resizeTo, resizeBy,
//   captureEvents, releaseEvents, find
//                                   No-ops; open() returns null, find() false.
//   getSelection                    An always-empty Selection.
//   navigator.sendBeacon            Returns true; nothing is sent.
//   navigator.clipboard             Writes are dropped; reads give nothing.
//   navigator.permissions.query     Resolves with state 'prompt'.
//   navigator.geolocation           Calls the error callback (PERMISSION_DENIED).
//   navigator.mediaDevices          No devices; getUserMedia rejects.
//   navigator.storage               estimate() reports nothing used;
//                                   persist()/persisted() resolve false.
//   navigator.connection            A fixed fast connection (4g, no saveData).
//   navigator.plugins, mimeTypes    Chrome's built-in PDF viewer entries.
//   navigator.vibrate, getGamepads, registerProtocolHandler,
//   unregisterProtocolHandler       No-ops.
//   Notification                    Permission stays 'default'; nothing shows.
//   ResizeObserver                  Reports each observed element once, with
//                                   its getBoundingClientRect() size if it has
//                                   one (the viewport size otherwise); no
//                                   later resizes, since nothing lays out.
//   PerformanceObserver             Never records entries.
//   BroadcastChannel                No other contexts exist; posts go nowhere.
//   WebSocket, EventSource          Fail to connect: an error event, then
//                                   close, as if the server were unreachable.
//   CSS.supports                    Answers true for everything.
//   crypto.subtle                   Every method rejects with NotSupportedError.
//   URL.createObjectURL             Returns a unique blob: URL that resolves to
//                                   nothing; revokeObjectURL is a no-op.
//
// Deliberately absent, so feature detection takes the fallback path:
// Worker, SharedWorker, navigator.serviceWorker, caches, indexedDB,
// speechSynthesis, PaymentRequest, WebGL/canvas contexts and WebRTC.
//
// Not stubs but real implementations elsewhere: CSS.escape (below),
// requestIdleCallback, matchMedia, storage, history and location (window.js,
// media.js, storage.js), and Intl (intl.js, which formats every locale as
// en-US).
(function (native, internals) {
  'use strict';

  const domException = internals.domException;
  const eventClass = internals.eventClass;
  const fire = internals.fire;
  const queueTask = internals.queueTask;
  const defineProperty = Object.defineProperty;
  const ReflectApply = Reflect.apply;
  const KEY = Symbol('stub');
  const cryptoRef = globalThis.crypto;
  const resolved = (value) => Promise.resolve(value);
  const noop = () => undefined;

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  function illegal(key) {
    if (key !== KEY) throw new TypeError('Illegal constructor');
  }

  function method(target, name, fn) {
    defineProperty(target, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }

  function getter(target, name, get) {
    defineProperty(target, name, { get, enumerable: true, configurable: true });
  }

  // Window functions.

  internals.expose({
    alert(message) {
      native.log('info', 'alert: ' + (message === undefined ? '' : String(message)));
    },
    confirm: () => false,
    prompt: () => null,
    print: noop,
    open: () => null,
    close: noop,
    stop: noop,
    focus: noop,
    blur: noop,
    moveTo: noop,
    moveBy: noop,
    resizeTo: noop,
    resizeBy: noop,
    captureEvents: noop,
    releaseEvents: noop,
    find: () => false,
  });

  class Selection {
    constructor(key) {
      illegal(key);
    }
    get anchorNode() { return null; }
    get anchorOffset() { return 0; }
    get focusNode() { return null; }
    get focusOffset() { return 0; }
    get isCollapsed() { return true; }
    get rangeCount() { return 0; }
    get type() { return 'None'; }
    get direction() { return 'none'; }
    getRangeAt() {
      throw domException('IndexSizeError', "Failed to execute 'getRangeAt' on 'Selection': 0 is not a valid index.");
    }
    toString() { return ''; }
  }
  for (const name of ['addRange', 'removeRange', 'removeAllRanges', 'empty', 'collapse', 'setPosition',
    'collapseToStart', 'collapseToEnd', 'extend', 'setBaseAndExtent', 'selectAllChildren',
    'deleteFromDocument', 'modify']) {
    method(Selection.prototype, name, noop);
  }
  method(Selection.prototype, 'containsNode', () => false);
  tag(Selection, 'Selection');
  const selection = new Selection(KEY);
  internals.expose({ Selection, getSelection: () => selection });

  // Navigator extras.

  const navigatorProto = Object.getPrototypeOf(navigator);
  method(navigatorProto, 'sendBeacon', function sendBeacon(url) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'sendBeacon' on 'Navigator': 1 argument required, but only 0 present.");
    }
    void url;
    return true;
  });
  method(navigatorProto, 'vibrate', () => true);
  method(navigatorProto, 'getGamepads', () => []);
  method(navigatorProto, 'registerProtocolHandler', noop);
  method(navigatorProto, 'unregisterProtocolHandler', noop);

  const clipboard = Object.freeze({
    writeText: () => resolved(undefined),
    write: () => resolved(undefined),
    readText: () => resolved(''),
    read: () => resolved([]),
  });
  getter(navigatorProto, 'clipboard', () => clipboard);

  class PermissionStatus extends EventTarget {
    #name;
    constructor(key, name) {
      illegal(key);
      super();
      this.#name = name;
    }
    get name() { return this.#name; }
    get state() { return 'prompt'; }
  }
  tag(PermissionStatus, 'PermissionStatus');
  internals.defineEventHandlers(PermissionStatus.prototype, ['onchange']);
  const permissions = Object.freeze({
    query(descriptor) {
      const name = descriptor && descriptor.name !== undefined ? String(descriptor.name) : '';
      return resolved(new PermissionStatus(KEY, name));
    },
  });
  getter(navigatorProto, 'permissions', () => permissions);

  const PERMISSION_DENIED = 1;
  function denyPosition(success, error) {
    if (typeof error === 'function') {
      queueTask(() => ReflectApply(error, undefined, [{
        code: PERMISSION_DENIED,
        message: 'User denied Geolocation',
        PERMISSION_DENIED: 1,
        POSITION_UNAVAILABLE: 2,
        TIMEOUT: 3,
      }]));
    }
  }
  let lastWatch = 0;
  const geolocation = Object.freeze({
    getCurrentPosition: denyPosition,
    watchPosition(success, error) {
      denyPosition(success, error);
      return ++lastWatch;
    },
    clearWatch: noop,
  });
  getter(navigatorProto, 'geolocation', () => geolocation);

  class MediaDevices extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    enumerateDevices() { return resolved([]); }
    getSupportedConstraints() { return {}; }
    getUserMedia() {
      return Promise.reject(domException('NotAllowedError', 'Permission denied'));
    }
    getDisplayMedia() {
      return Promise.reject(domException('NotAllowedError', 'Permission denied'));
    }
  }
  tag(MediaDevices, 'MediaDevices');
  internals.defineEventHandlers(MediaDevices.prototype, ['ondevicechange']);
  const mediaDevices = new MediaDevices(KEY);
  getter(navigatorProto, 'mediaDevices', () => mediaDevices);

  const storageManager = Object.freeze({
    estimate: () => resolved({ quota: 1024 * 1024 * 1024, usage: 0, usageDetails: {} }),
    persist: () => resolved(false),
    persisted: () => resolved(false),
    getDirectory: () => Promise.reject(domException('SecurityError', 'Storage directory access is denied.')),
  });
  getter(navigatorProto, 'storage', () => storageManager);

  class NetworkInformation extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get effectiveType() { return '4g'; }
    get downlink() { return 10; }
    get rtt() { return 50; }
    get saveData() { return false; }
  }
  tag(NetworkInformation, 'NetworkInformation');
  internals.defineEventHandlers(NetworkInformation.prototype, ['onchange']);
  const connection = new NetworkInformation(KEY);
  getter(navigatorProto, 'connection', () => connection);

  // Chrome lists its built-in PDF viewer under five names.
  const pdfMimeTypes = [];
  const plugins = [];
  for (const name of ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer',
    'Microsoft Edge PDF Viewer', 'WebKit built-in PDF']) {
    plugins.push({ name, filename: 'internal-pdf-viewer', description: 'Portable Document Format' });
  }
  for (const type of ['application/pdf', 'text/pdf']) {
    pdfMimeTypes.push({ type, suffixes: 'pdf', description: 'Portable Document Format', enabledPlugin: null });
  }

  function arrayLike(items, Tag, keyOf, extra) {
    const list = Object.create(Tag.prototype);
    items.forEach((item, i) => {
      defineProperty(list, i, { value: item, enumerable: true });
      defineProperty(list, keyOf(item), { value: item });
    });
    defineProperty(list, 'length', { value: items.length });
    if (extra) extra(list);
    return Object.freeze(list);
  }

  class MimeType {
    constructor(key) { illegal(key); }
  }
  class Plugin {
    constructor(key) { illegal(key); }
  }
  class MimeTypeArray {
    constructor(key) { illegal(key); }
    item(i) { return this[i >>> 0] || null; }
    namedItem(name) { return Object.prototype.hasOwnProperty.call(this, name) ? this[name] : null; }
    *[Symbol.iterator]() { for (let i = 0; i < this.length; i++) yield this[i]; }
  }
  class PluginArray {
    constructor(key) { illegal(key); }
    item(i) { return this[i >>> 0] || null; }
    namedItem(name) { return Object.prototype.hasOwnProperty.call(this, name) ? this[name] : null; }
    refresh() {}
    *[Symbol.iterator]() { for (let i = 0; i < this.length; i++) yield this[i]; }
  }
  for (const [C, name] of [[MimeType, 'MimeType'], [Plugin, 'Plugin'], [MimeTypeArray, 'MimeTypeArray'], [PluginArray, 'PluginArray']]) {
    tag(C, name);
  }
  const firstPlugin = { value: null };
  const mimeTypeObjects = pdfMimeTypes.map((m) => {
    const o = Object.create(MimeType.prototype);
    for (const key of ['type', 'suffixes', 'description']) defineProperty(o, key, { value: m[key], enumerable: true });
    getter(o, 'enabledPlugin', () => firstPlugin.value);
    return Object.freeze(o);
  });
  const pluginObjects = plugins.map((p) => {
    const o = Object.create(Plugin.prototype);
    for (const key of ['name', 'filename', 'description']) defineProperty(o, key, { value: p[key], enumerable: true });
    mimeTypeObjects.forEach((m, i) => defineProperty(o, i, { value: m, enumerable: true }));
    defineProperty(o, 'length', { value: mimeTypeObjects.length });
    method(o, 'item', (i) => mimeTypeObjects[i >>> 0] || null);
    method(o, 'namedItem', (type) => mimeTypeObjects.find((m) => m.type === type) || null);
    return Object.freeze(o);
  });
  firstPlugin.value = pluginObjects[0];
  const pluginArray = arrayLike(pluginObjects, PluginArray, (p) => p.name);
  const mimeTypeArray = arrayLike(mimeTypeObjects, MimeTypeArray, (m) => m.type);
  getter(navigatorProto, 'plugins', () => pluginArray);
  getter(navigatorProto, 'mimeTypes', () => mimeTypeArray);

  // Notification.

  class Notification extends EventTarget {
    #title;
    #options;
    constructor(title, options) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'Notification': 1 argument required, but only 0 present.");
      }
      super();
      this.#title = String(title);
      this.#options = options === undefined || options === null ? {} : options;
      queueTask(() => fire(this, 'error'));
    }
    static get permission() { return 'default'; }
    static get maxActions() { return 2; }
    static requestPermission(callback) {
      if (typeof callback === 'function') queueTask(() => callback('default'));
      return resolved('default');
    }
    get title() { return this.#title; }
    get body() { return this.#options.body === undefined ? '' : String(this.#options.body); }
    get tag() { return this.#options.tag === undefined ? '' : String(this.#options.tag); }
    get data() { return this.#options.data === undefined ? null : this.#options.data; }
    close() {}
  }
  tag(Notification, 'Notification');
  internals.defineEventHandlers(Notification.prototype, ['onclick', 'onshow', 'onerror', 'onclose']);

  // ResizeObserver: one report per observed element.

  function boxOf(target) {
    let rect = null;
    if (target !== null && typeof target === 'object' && typeof target.getBoundingClientRect === 'function') {
      try {
        rect = target.getBoundingClientRect();
      } catch (_) {
        rect = null;
      }
    }
    const width = rect ? Number(rect.width) || 0 : innerWidth;
    const height = rect ? Number(rect.height) || 0 : innerHeight;
    return { width, height };
  }

  class ResizeObserverEntry {
    #target;
    #width;
    #height;
    constructor(key, target, width, height) {
      illegal(key);
      this.#target = target;
      this.#width = width;
      this.#height = height;
    }
    get target() { return this.#target; }
    get contentRect() {
      const w = this.#width;
      const h = this.#height;
      return { x: 0, y: 0, width: w, height: h, top: 0, left: 0, right: w, bottom: h };
    }
    get borderBoxSize() { return Object.freeze([Object.freeze({ inlineSize: this.#width, blockSize: this.#height })]); }
    get contentBoxSize() { return this.borderBoxSize; }
    get devicePixelContentBoxSize() {
      const dpr = devicePixelRatio;
      return Object.freeze([Object.freeze({ inlineSize: this.#width * dpr, blockSize: this.#height * dpr })]);
    }
  }
  tag(ResizeObserverEntry, 'ResizeObserverEntry');

  class ResizeObserver {
    #callback;
    #pending = new Set();
    #observed = new Set();
    #queued = false;
    constructor(callback) {
      if (typeof callback !== 'function') {
        throw new TypeError("Failed to construct 'ResizeObserver': The callback provided as parameter 1 is not a function.");
      }
      this.#callback = callback;
    }
    observe(target) {
      if (arguments.length === 0 || target === null || typeof target !== 'object') {
        throw new TypeError("Failed to execute 'observe' on 'ResizeObserver': parameter 1 is not of type 'Element'.");
      }
      if (this.#observed.has(target)) return;
      this.#observed.add(target);
      this.#pending.add(target);
      if (this.#queued) return;
      this.#queued = true;
      queueTask(() => {
        this.#queued = false;
        const targets = [...this.#pending];
        this.#pending.clear();
        if (targets.length === 0) return;
        const entries = targets.map((t) => {
          const box = boxOf(t);
          return new ResizeObserverEntry(KEY, t, box.width, box.height);
        });
        ReflectApply(this.#callback, this, [entries, this]);
      });
    }
    unobserve(target) {
      this.#observed.delete(target);
      this.#pending.delete(target);
    }
    disconnect() {
      this.#observed.clear();
      this.#pending.clear();
    }
  }
  tag(ResizeObserver, 'ResizeObserver');

  // PerformanceObserver.

  class PerformanceObserver {
    constructor(callback) {
      if (typeof callback !== 'function') {
        throw new TypeError("Failed to construct 'PerformanceObserver': The callback provided as parameter 1 is not a function.");
      }
    }
    static get supportedEntryTypes() { return Object.freeze([]); }
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  }
  tag(PerformanceObserver, 'PerformanceObserver');

  // BroadcastChannel: there are no other contexts to talk to.

  class BroadcastChannel extends EventTarget {
    #name;
    #closed = false;
    constructor(name) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
      }
      super();
      this.#name = String(name);
    }
    get name() { return this.#name; }
    postMessage(message) {
      if (this.#closed) {
        throw domException('InvalidStateError', "Failed to execute 'postMessage' on 'BroadcastChannel': Channel is closed");
      }
      void message;
    }
    close() {
      this.#closed = true;
    }
  }
  tag(BroadcastChannel, 'BroadcastChannel');
  internals.defineEventHandlers(BroadcastChannel.prototype, ['onmessage', 'onmessageerror']);

  // WebSocket and EventSource fail as if the server were unreachable.

  const CloseEvent = eventClass('CloseEvent', { wasClean: false, code: 0, reason: '' });

  function streamUrl(url, schemes, what) {
    const parts = internals.urlParse(url, internals.baseURL());
    if (parts === undefined) {
      throw domException('SyntaxError', 'Failed to construct ' + what + ": The URL '" + String(url) + "' is invalid.");
    }
    let href = parts[internals.URL_PARTS.HREF];
    const protocol = parts[internals.URL_PARTS.PROTOCOL];
    if (what === "'WebSocket'" && (protocol === 'http:' || protocol === 'https:')) {
      href = (protocol === 'http:' ? 'ws' : 'wss') + href.slice(protocol.length - 1);
    } else if (!schemes.includes(protocol)) {
      throw domException('SyntaxError', 'Failed to construct ' + what + ": The URL's scheme must be one of " + schemes.join(', '));
    }
    return href;
  }

  class WebSocket extends EventTarget {
    #url;
    #state = 0;
    #binaryType = 'blob';
    constructor(url, protocols) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
      }
      super();
      void protocols;
      this.#url = streamUrl(url, ['ws:', 'wss:'], "'WebSocket'");
      queueTask(() => {
        if (this.#state === 3) return;
        this.#state = 3;
        fire(this, 'error');
        fire(this, 'close', { wasClean: false, code: 1006, reason: '' }, CloseEvent);
      });
    }
    get url() { return this.#url; }
    get readyState() { return this.#state; }
    get bufferedAmount() { return 0; }
    get extensions() { return ''; }
    get protocol() { return ''; }
    get binaryType() { return this.#binaryType; }
    set binaryType(value) {
      if (value === 'blob' || value === 'arraybuffer') this.#binaryType = value;
    }
    send() {
      if (this.#state === 0) {
        throw domException('InvalidStateError', "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.");
      }
    }
    close() {
      if (this.#state >= 2) return;
      this.#state = 3;
      queueTask(() => fire(this, 'close', { wasClean: false, code: 1006, reason: '' }, CloseEvent));
    }
  }
  for (const [name, value] of [['CONNECTING', 0], ['OPEN', 1], ['CLOSING', 2], ['CLOSED', 3]]) {
    defineProperty(WebSocket, name, { value, enumerable: true });
    defineProperty(WebSocket.prototype, name, { value, enumerable: true });
  }
  tag(WebSocket, 'WebSocket');
  internals.defineEventHandlers(WebSocket.prototype, ['onopen', 'onmessage', 'onerror', 'onclose']);

  class EventSource extends EventTarget {
    #url;
    #state = 0;
    #withCredentials;
    constructor(url, init) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'EventSource': 1 argument required, but only 0 present.");
      }
      super();
      this.#url = streamUrl(url, ['http:', 'https:'], "'EventSource'");
      this.#withCredentials = !!(init && init.withCredentials);
      queueTask(() => {
        if (this.#state === 2) return;
        this.#state = 2;
        fire(this, 'error');
      });
    }
    get url() { return this.#url; }
    get readyState() { return this.#state; }
    get withCredentials() { return this.#withCredentials; }
    close() {
      this.#state = 2;
    }
  }
  for (const [name, value] of [['CONNECTING', 0], ['OPEN', 1], ['CLOSED', 2]]) {
    defineProperty(EventSource, name, { value, enumerable: true });
    defineProperty(EventSource.prototype, name, { value, enumerable: true });
  }
  tag(EventSource, 'EventSource');
  internals.defineEventHandlers(EventSource.prototype, ['onopen', 'onmessage', 'onerror']);

  // CSS namespace. escape() is real; supports() always says yes.
  // https://drafts.csswg.org/cssom/#the-css.escape()-method

  function cssEscape(value) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'escape' on 'CSS': 1 argument required, but only 0 present.");
    }
    const string = String(value);
    let result = '';
    const first = string.charCodeAt(0);
    for (let i = 0; i < string.length; i++) {
      const c = string.charCodeAt(i);
      if (c === 0) {
        result += '�';
      } else if ((c >= 0x1 && c <= 0x1f) || c === 0x7f ||
        (i === 0 && c >= 0x30 && c <= 0x39) ||
        (i === 1 && c >= 0x30 && c <= 0x39 && first === 0x2d)) {
        result += '\\' + c.toString(16) + ' ';
      } else if (i === 0 && c === 0x2d && string.length === 1) {
        result += '\\' + string.charAt(i);
      } else if (c >= 0x80 || c === 0x2d || c === 0x5f || (c >= 0x30 && c <= 0x39) ||
        (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a)) {
        result += string.charAt(i);
      } else {
        result += '\\' + string.charAt(i);
      }
    }
    return result;
  }

  const CSS = Object.freeze({
    escape: cssEscape,
    supports: () => true,
    registerProperty: noop,
    [Symbol.toStringTag]: 'CSS',
  });

  // crypto.subtle.

  class SubtleCrypto {
    constructor(key) {
      illegal(key);
    }
  }
  for (const name of ['encrypt', 'decrypt', 'sign', 'verify', 'digest', 'generateKey', 'deriveKey',
    'deriveBits', 'importKey', 'exportKey', 'wrapKey', 'unwrapKey']) {
    method(SubtleCrypto.prototype, name, function () {
      return Promise.reject(domException('NotSupportedError', "Failed to execute '" + name + "' on 'SubtleCrypto': not supported."));
    });
  }
  tag(SubtleCrypto, 'SubtleCrypto');
  const subtle = new SubtleCrypto(KEY);
  getter(Crypto.prototype, 'subtle', () => subtle);

  // Blob URLs.

  method(URL, 'createObjectURL', function createObjectURL(object) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'createObjectURL' on 'URL': 1 argument required, but only 0 present.");
    }
    void object;
    const origin = internals.origin();
    return 'blob:' + (origin === 'null' ? 'null' : origin) + '/' + cryptoRef.randomUUID();
  });
  method(URL, 'revokeObjectURL', noop);

  internals.expose({
    Notification, ResizeObserver, ResizeObserverEntry, PerformanceObserver, BroadcastChannel,
    CloseEvent, WebSocket, EventSource, CSS, SubtleCrypto, PermissionStatus, MediaDevices,
    NetworkInformation, Plugin, PluginArray, MimeType, MimeTypeArray,
  });
})
