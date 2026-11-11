// DOM events: DOMException, Event, CustomEvent, EventTarget, event handler
// attributes, AbortController and AbortSignal.
// https://dom.spec.whatwg.org/#events
// https://webidl.spec.whatwg.org/#idl-DOMException
//
// Like every glue file, this evaluates to a function that receives the
// `native` host functions and the shared `internals` object, which later
// glue files (and the DOM bindings) use to dispatch trusted events, define
// `on*` attributes and hook into abort signals.
(function (native, internals) {
  'use strict';

  const ReflectApply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const reportError = native.reportError;
  const pageNow = native.now;
  const queueTaskNative = native.queueTask;
  const setTimeoutRef = globalThis.setTimeout;

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to ' + what + ': ' + n + ' argument' + (n === 1 ? '' : 's') +
        ' required, but only ' + args.length + ' present.');
    }
  }

  function dictionary(value, what) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError('Failed to ' + what + ": The provided value is not of type 'Dictionary'.");
    }
    return value;
  }

  // Queues `callback` as its own task on the event loop.
  function queueTask(callback) {
    queueTaskNative(callback);
  }

  // DOMException. https://webidl.spec.whatwg.org/#idl-DOMException

  const LEGACY_CODES = [
    ['IndexSizeError', 'INDEX_SIZE_ERR', 1],
    ['', 'DOMSTRING_SIZE_ERR', 2],
    ['HierarchyRequestError', 'HIERARCHY_REQUEST_ERR', 3],
    ['WrongDocumentError', 'WRONG_DOCUMENT_ERR', 4],
    ['InvalidCharacterError', 'INVALID_CHARACTER_ERR', 5],
    ['', 'NO_DATA_ALLOWED_ERR', 6],
    ['NoModificationAllowedError', 'NO_MODIFICATION_ALLOWED_ERR', 7],
    ['NotFoundError', 'NOT_FOUND_ERR', 8],
    ['NotSupportedError', 'NOT_SUPPORTED_ERR', 9],
    ['InUseAttributeError', 'INUSE_ATTRIBUTE_ERR', 10],
    ['InvalidStateError', 'INVALID_STATE_ERR', 11],
    ['SyntaxError', 'SYNTAX_ERR', 12],
    ['InvalidModificationError', 'INVALID_MODIFICATION_ERR', 13],
    ['NamespaceError', 'NAMESPACE_ERR', 14],
    ['InvalidAccessError', 'INVALID_ACCESS_ERR', 15],
    ['', 'VALIDATION_ERR', 16],
    ['TypeMismatchError', 'TYPE_MISMATCH_ERR', 17],
    ['SecurityError', 'SECURITY_ERR', 18],
    ['NetworkError', 'NETWORK_ERR', 19],
    ['AbortError', 'ABORT_ERR', 20],
    ['URLMismatchError', 'URL_MISMATCH_ERR', 21],
    ['QuotaExceededError', 'QUOTA_EXCEEDED_ERR', 22],
    ['TimeoutError', 'TIMEOUT_ERR', 23],
    ['InvalidNodeTypeError', 'INVALID_NODE_TYPE_ERR', 24],
    ['DataCloneError', 'DATA_CLONE_ERR', 25],
  ];
  const codes = new Map();
  for (const [name, , code] of LEGACY_CODES) if (name) codes.set(name, code);

  class DOMException extends Error {
    #name;
    #message;
    constructor(message = '', name = 'Error') {
      super();
      this.#message = String(message);
      this.#name = String(name);
    }
    get name() {
      return this.#name;
    }
    get message() {
      return this.#message;
    }
    get code() {
      return codes.get(this.#name) || 0;
    }
  }
  for (const [, constant, code] of LEGACY_CODES) {
    const desc = { value: code, enumerable: true };
    defineProperty(DOMException, constant, desc);
    defineProperty(DOMException.prototype, constant, desc);
  }
  tag(DOMException, 'DOMException');

  function domException(name, message) {
    return new DOMException(message, name);
  }

  // Event. https://dom.spec.whatwg.org/#interface-event

  const NONE = 0;
  const CAPTURING_PHASE = 1;
  const AT_TARGET = 2;
  const BUBBLING_PHASE = 3;

  let eventState;
  let isEvent;

  class Event {
    #s;
    constructor(type, eventInitDict) {
      requireArgs(arguments, 1, "construct 'Event'");
      const init = dictionary(eventInitDict, "construct 'Event'");
      this.#s = {
        type: String(type),
        bubbles: !!init.bubbles,
        cancelable: !!init.cancelable,
        composed: !!init.composed,
        target: null,
        currentTarget: null,
        phase: NONE,
        canceled: false,
        stop: false,
        stopImmediate: false,
        inPassiveListener: false,
        dispatching: false,
        trusted: false,
        timeStamp: pageNow(),
        path: null,
      };
    }
    static {
      eventState = (event) => event.#s;
      isEvent = (value) => typeof value === 'object' && value !== null && #s in value;
    }
    get type() { return this.#s.type; }
    get target() { return this.#s.target; }
    get srcElement() { return this.#s.target; }
    get currentTarget() { return this.#s.currentTarget; }
    get eventPhase() { return this.#s.phase; }
    get bubbles() { return this.#s.bubbles; }
    get cancelable() { return this.#s.cancelable; }
    get composed() { return this.#s.composed; }
    get defaultPrevented() { return this.#s.canceled; }
    get isTrusted() { return this.#s.trusted; }
    get timeStamp() { return this.#s.timeStamp; }
    get returnValue() { return !this.#s.canceled; }
    set returnValue(value) {
      if (!value) this.preventDefault();
    }
    get cancelBubble() { return this.#s.stop; }
    set cancelBubble(value) {
      if (value) this.#s.stop = true;
    }
    composedPath() {
      const s = this.#s;
      return s.dispatching && s.path ? s.path.slice() : [];
    }
    stopPropagation() {
      this.#s.stop = true;
    }
    stopImmediatePropagation() {
      this.#s.stop = true;
      this.#s.stopImmediate = true;
    }
    preventDefault() {
      const s = this.#s;
      if (s.cancelable && !s.inPassiveListener) s.canceled = true;
    }
    initEvent(type, bubbles = false, cancelable = false) {
      requireArgs(arguments, 1, "execute 'initEvent' on 'Event'");
      const s = this.#s;
      if (s.dispatching) return;
      s.type = String(type);
      s.bubbles = !!bubbles;
      s.cancelable = !!cancelable;
      s.canceled = false;
      s.stop = false;
      s.stopImmediate = false;
      s.trusted = false;
      s.target = null;
    }
  }
  for (const [name, value] of [
    ['NONE', NONE],
    ['CAPTURING_PHASE', CAPTURING_PHASE],
    ['AT_TARGET', AT_TARGET],
    ['BUBBLING_PHASE', BUBBLING_PHASE],
  ]) {
    defineProperty(Event, name, { value, enumerable: true });
    defineProperty(Event.prototype, name, { value, enumerable: true });
  }
  tag(Event, 'Event');

  // Builds an Event subclass whose extra init-dictionary members become
  // read-only attributes, defaulting to the values in `defaults`.
  function eventClass(name, defaults, Base = Event) {
    const keys = Object.keys(defaults);
    const what = "construct '" + name + "'";
    const C = class extends Base {
      #fields;
      constructor(type, eventInitDict) {
        requireArgs(arguments, 1, what);
        const init = dictionary(eventInitDict, what);
        super(type, init);
        const fields = {};
        for (const key of keys) {
          const value = init[key];
          fields[key] = value === undefined ? defaults[key] : value;
        }
        this.#fields = fields;
      }
      static {
        for (const key of keys) {
          defineProperty(this.prototype, key, {
            get() {
              return this.#fields[key];
            },
            configurable: true,
            enumerable: true,
          });
        }
      }
    };
    defineProperty(C, 'name', { value: name });
    tag(C, name);
    return C;
  }

  const CustomEvent = eventClass('CustomEvent', { detail: null });
  CustomEvent.prototype.initCustomEvent = function initCustomEvent(type, bubbles, cancelable, detail) {
    this.initEvent(type, bubbles, cancelable);
    // The detail is fixed at construction; legacy initialization only
    // updates the base event, which is all pages rely on in practice.
    void detail;
  };
  const ErrorEvent = eventClass('ErrorEvent', {
    message: '',
    filename: '',
    lineno: 0,
    colno: 0,
    error: undefined,
  });
  const PromiseRejectionEvent = eventClass('PromiseRejectionEvent', {
    promise: undefined,
    reason: undefined,
  });

  // EventTarget. https://dom.spec.whatwg.org/#interface-eventtarget
  //
  // Listeners live in a WeakMap keyed by target, so any object whose
  // prototype chain reaches EventTarget.prototype (the global object, DOM
  // nodes built by the host) can take listeners without per-object setup.

  const listenerMaps = new WeakMap();

  function listenersFor(target, type, create) {
    let map = listenerMaps.get(target);
    if (map === undefined) {
      if (!create) return undefined;
      map = new Map();
      listenerMaps.set(target, map);
    }
    let list = map.get(type);
    if (list === undefined && create) {
      list = [];
      map.set(type, list);
    }
    return list;
  }

  function flattenCapture(options) {
    return typeof options === 'object' && options !== null ? !!options.capture : !!options;
  }

  function removeListener(target, type, listener) {
    listener.removed = true;
    const list = listenersFor(target, type, false);
    if (list === undefined) return;
    const i = list.indexOf(listener);
    if (i >= 0) list.splice(i, 1);
  }

  function addListener(target, type, callback, capture, once, passive, signal) {
    const list = listenersFor(target, type, true);
    for (const l of list) {
      if (l.callback === callback && l.capture === capture) return;
    }
    const listener = { callback, capture, once, passive, removed: false };
    list.push(listener);
    if (signal !== undefined) {
      signalAddAlgorithm(signal, () => removeListener(target, type, listener));
    }
  }

  function receiver(self) {
    return self === undefined || self === null ? globalThis : self;
  }

  class EventTarget {
    addEventListener(type, callback, options) {
      requireArgs(arguments, 2, "execute 'addEventListener' on 'EventTarget'");
      const target = receiver(this);
      if (callback === null || callback === undefined) return;
      if (typeof callback !== 'function' && typeof callback !== 'object') {
        throw new TypeError("Failed to execute 'addEventListener' on 'EventTarget': parameter 2 is not of type 'Object'.");
      }
      let once = false;
      let passive = false;
      let signal;
      if (typeof options === 'object' && options !== null) {
        once = !!options.once;
        passive = !!options.passive;
        if (options.signal !== undefined) {
          signal = options.signal;
          if (!isSignal(signal)) {
            throw new TypeError("Failed to execute 'addEventListener' on 'EventTarget': member signal is not of type 'AbortSignal'.");
          }
          if (signalState(signal).aborted) return;
        }
      }
      addListener(target, String(type), callback, flattenCapture(options), once, passive, signal);
    }

    removeEventListener(type, callback, options) {
      requireArgs(arguments, 2, "execute 'removeEventListener' on 'EventTarget'");
      const target = receiver(this);
      type = String(type);
      const capture = flattenCapture(options);
      const list = listenersFor(target, type, false);
      if (list === undefined) return;
      const listener = list.find((l) => l.callback === callback && l.capture === capture);
      if (listener !== undefined) removeListener(target, type, listener);
    }

    dispatchEvent(event) {
      requireArgs(arguments, 1, "execute 'dispatchEvent' on 'EventTarget'");
      if (!isEvent(event)) {
        throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.");
      }
      const s = eventState(event);
      if (s.dispatching) {
        throw new DOMException("Failed to execute 'dispatchEvent' on 'EventTarget': The event is already being dispatched.", 'InvalidStateError');
      }
      return dispatch(receiver(this), event, false);
    }
  }
  tag(EventTarget, 'EventTarget');

  function invoke(target, event, s, phase, capture) {
    // Event handlers are bubble-phase listeners; one set by a content
    // attribute nothing has read yet joins the list here.
    if (!capture) existingHandler(target, s.type);
    const list = listenersFor(target, s.type, false);
    if (list === undefined || list.length === 0) return;
    s.currentTarget = target;
    s.phase = phase;
    for (const listener of list.slice()) {
      if (listener.removed || listener.capture !== capture) continue;
      if (listener.once) removeListener(target, s.type, listener);
      s.inPassiveListener = listener.passive;
      try {
        const callback = listener.callback;
        if (typeof callback === 'function') {
          ReflectApply(callback, target, [event]);
        } else {
          const handleEvent = callback.handleEvent;
          if (typeof handleEvent !== 'function') {
            throw new TypeError("The provided callback is not a function and has no 'handleEvent' method.");
          }
          ReflectApply(handleEvent, callback, [event]);
        }
      } catch (error) {
        reportError(error);
      }
      s.inPassiveListener = false;
      if (s.stopImmediate) break;
    }
  }

  // Dispatches `event` at `target` through the capture, target and bubble
  // phases. The path above `target` comes from `internals.eventParent`.
  // https://dom.spec.whatwg.org/#concept-event-dispatch
  function dispatch(target, event, trusted = true) {
    const s = eventState(event);
    s.trusted = trusted;
    s.dispatching = true;
    s.target = target;
    const path = [target];
    const parentOf = internals.eventParent;
    for (let p = parentOf(target, event); p !== null && p !== undefined; p = parentOf(p, event)) {
      path.push(p);
    }
    s.path = path;
    try {
      for (let i = path.length - 1; i > 0 && !s.stop; i--) invoke(path[i], event, s, CAPTURING_PHASE, true);
      if (!s.stop) invoke(target, event, s, AT_TARGET, true);
      if (!s.stop) invoke(target, event, s, AT_TARGET, false);
      if (s.bubbles) {
        for (let i = 1; i < path.length && !s.stop; i++) invoke(path[i], event, s, BUBBLING_PHASE, false);
      }
    } finally {
      s.phase = NONE;
      s.currentTarget = null;
      s.dispatching = false;
      s.stop = false;
      s.stopImmediate = false;
      s.path = null;
    }
    return !s.canceled;
  }

  // Creates and dispatches a trusted event.
  function fire(target, type, init, Ctor = Event) {
    return dispatch(target, new Ctor(type, init), true);
  }

  // Reporting exceptions to the page.
  // https://html.spec.whatwg.org/#report-an-exception

  // One stack frame: `at fn (file:line:col)` or `at file:line:col`.
  const STACK_FRAME = /^\s*at (?:.*? \()?(.+?):(\d+):(\d+)\)?$/;
  let reportingException = false;

  // The script location an exception was thrown from, read off the first
  // stack frame that has one, as [filename, lineno, colno].
  function errorLocation(error) {
    try {
      const stack = error instanceof Error ? error.stack : undefined;
      if (typeof stack === 'string') {
        for (const line of stack.split('\n')) {
          const m = STACK_FRAME.exec(line);
          if (m !== null) return [m[1], +m[2], +m[3]];
        }
      }
    } catch (_) {
      // A throwing `stack` getter leaves the location unknown.
    }
    return ['', 0, 0];
  }

  function describeError(error) {
    try {
      return String(error);
    } catch (_) {
      return 'exception';
    }
  }

  // Fires `error` at the global for an exception nobody caught. Returns
  // whether a listener canceled it (or `onerror` returned true), in which
  // case the exception counts as handled. An exception reported while the
  // event is being dispatched gets no event of its own, so a throwing
  // error handler cannot recurse.
  function reportException(error) {
    if (reportingException) return false;
    reportingException = true;
    try {
      const [filename, lineno, colno] = errorLocation(error);
      return !fire(globalThis, 'error', {
        cancelable: true,
        message: 'Uncaught ' + describeError(error),
        filename,
        lineno,
        colno,
        error,
      }, ErrorEvent);
    } finally {
      reportingException = false;
    }
  }

  // Promise rejections. Rust tracks which rejected promises have no
  // handler and calls these at the end of microtask checkpoints.
  // https://html.spec.whatwg.org/#unhandled-promise-rejections

  // Promises whose rejection was reported as unhandled, mapped to their
  // reasons, until a handler is attached.
  const outstandingRejections = new WeakMap();

  // Fires `unhandledrejection` at the global. Returns whether a listener
  // canceled it, which marks the rejection handled.
  function unhandledRejection(promise, reason) {
    return !fire(globalThis, 'unhandledrejection', { cancelable: true, promise, reason },
      PromiseRejectionEvent);
  }

  function rejectionOutstanding(promise, reason) {
    outstandingRejections.set(promise, reason);
  }

  // A handler was attached to `promise` after its rejection was reported:
  // queues a task to fire `rejectionhandled`.
  function rejectionHandled(promise) {
    if (!outstandingRejections.has(promise)) return;
    const reason = outstandingRejections.get(promise);
    outstandingRejections.delete(promise);
    queueTask(() => {
      fire(globalThis, 'rejectionhandled', { promise, reason }, PromiseRejectionEvent);
    });
  }

  // Event handlers: the `on*` IDL attributes (`onload`, `onabort`, ...)
  // and, for elements, the `on*` content attributes that set them.
  // https://html.spec.whatwg.org/#event-handler-attributes
  //
  // A handler is a record { value, source, owner, listener } per target and
  // type. `value` is the function, or `source` the uncompiled body of a
  // content attribute on the element `owner`, compiled the first time the
  // handler runs or is read. `listener` is the handler's place among the
  // target's event listeners while it is active.
  //
  // Content attributes reach here two ways. Changes made through the DOM
  // call `attributeHandlerChanged`. Attributes the element was created with
  // (by the parser, innerHTML or cloning) are only looked at when the
  // handler is first needed, through `internals.handlerAttribute`; since
  // they were set before script could add any listener, their handler goes
  // first in the list.

  const handlerMaps = new WeakMap();

  function handlerRecord(target, type, create) {
    let map = handlerMaps.get(target);
    if (map === undefined) {
      if (!create) return undefined;
      map = new Map();
      handlerMaps.set(target, map);
    }
    let handler = map.get(type);
    if (handler === undefined && create) {
      handler = { value: null, source: null, owner: null, listener: null };
      map.set(type, handler);
    }
    return handler;
  }

  function activateHandler(target, type, handler, first) {
    if (handler.listener !== null) return;
    const listener = {
      callback: (event) => runHandler(handler, target, event),
      capture: false,
      once: false,
      passive: false,
      removed: false,
    };
    const list = listenersFor(target, type, true);
    if (first) list.unshift(listener);
    else list.push(listener);
    handler.listener = listener;
  }

  function deactivateHandler(target, type, handler) {
    handler.value = null;
    handler.source = null;
    handler.owner = null;
    if (handler.listener !== null) removeListener(target, type, handler.listener);
    handler.listener = null;
  }

  // The handler for `type` on `target`, if it has one, picking up a content
  // attribute nothing has looked at yet.
  function existingHandler(target, type) {
    const handler = handlerRecord(target, type, false);
    if (handler !== undefined) return handler;
    const attribute = internals.handlerAttribute(target, type);
    if (attribute === null) return undefined;
    const created = handlerRecord(target, type, true);
    created.owner = attribute[0];
    created.source = attribute[1];
    activateHandler(target, type, created, true);
    return created;
  }

  // The content attribute that sets the handler for `type` on `target` was
  // set to `value` on `owner`, or removed if `value` is null. Call it once
  // the attribute has changed, and `existingHandler` just before, so an
  // attribute the element was created with keeps its place.
  // https://html.spec.whatwg.org/#event-handler-attributes:concept-element-attributes-change-ext
  function attributeHandlerChanged(target, type, owner, value) {
    const handler = handlerRecord(target, type, value !== null);
    if (handler === undefined) return;
    if (value === null) {
      deactivateHandler(target, type, handler);
      return;
    }
    handler.value = null;
    handler.source = value;
    handler.owner = owner;
    activateHandler(target, type, handler, false);
  }

  // The handler's function, compiling an uncompiled one. A body that does
  // not compile is reported and leaves the handler null.
  // https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler
  function handlerValue(handler, target, type) {
    if (handler.source !== null) {
      const source = handler.source;
      handler.source = null;
      try {
        handler.value = internals.compileHandler(handler.owner, type, source, target === globalThis);
      } catch (error) {
        handler.value = null;
        reportError(error);
      }
      handler.owner = null;
    }
    return handler.value;
  }

  function runHandler(handler, target, event) {
    const callback = handlerValue(handler, target, event.type);
    if (callback === null) return;
    let result;
    if (event.type === 'error' && target === globalThis && event instanceof ErrorEvent) {
      result = ReflectApply(callback, target,
        [event.message, event.filename, event.lineno, event.colno, event.error]);
      if (result === true) event.preventDefault();
      return;
    }
    result = ReflectApply(callback, target, [event]);
    if (result === false) event.preventDefault();
  }

  function defineEventHandler(proto, name) {
    const type = name.slice(2);
    defineProperty(proto, name, {
      get() {
        const target = receiver(this);
        const handler = existingHandler(target, type);
        return handler === undefined ? null : handlerValue(handler, target, type);
      },
      set(value) {
        const target = receiver(this);
        const handler = existingHandler(target, type) || handlerRecord(target, type, true);
        if (typeof value === 'function') {
          handler.value = value;
          handler.source = null;
          handler.owner = null;
          activateHandler(target, type, handler, false);
        } else {
          deactivateHandler(target, type, handler);
        }
      },
      configurable: true,
      enumerable: true,
    });
  }

  function defineEventHandlers(proto, names) {
    for (const name of names) defineEventHandler(proto, name);
  }

  // AbortSignal and AbortController.
  // https://dom.spec.whatwg.org/#interface-abortsignal

  const SIGNAL_KEY = Symbol('AbortSignal');
  let signalState;
  let isSignal;

  class AbortSignal extends EventTarget {
    #s = { aborted: false, reason: undefined, algorithms: new Set(), dependents: [] };
    constructor(key) {
      super();
      if (key !== SIGNAL_KEY) throw new TypeError('Illegal constructor');
    }
    static {
      signalState = (signal) => signal.#s;
      isSignal = (value) => typeof value === 'object' && value !== null && #s in value;
    }
    get aborted() {
      return this.#s.aborted;
    }
    get reason() {
      return this.#s.reason;
    }
    throwIfAborted() {
      if (this.#s.aborted) throw this.#s.reason;
    }
    static abort(reason) {
      const signal = new AbortSignal(SIGNAL_KEY);
      const s = signalState(signal);
      s.aborted = true;
      s.reason = reason === undefined ? abortError() : reason;
      return signal;
    }
    static timeout(milliseconds) {
      requireArgs(arguments, 1, "execute 'timeout' on 'AbortSignal'");
      const ms = Number(milliseconds);
      if (!Number.isFinite(ms) || ms < 0) {
        throw new TypeError("Failed to execute 'timeout' on 'AbortSignal': Value is outside the 'unsigned long long' value range.");
      }
      const signal = new AbortSignal(SIGNAL_KEY);
      ReflectApply(setTimeoutRef, globalThis, [() => {
        signalAbort(signal, new DOMException('signal timed out', 'TimeoutError'));
      }, ms]);
      return signal;
    }
    static any(signals) {
      requireArgs(arguments, 1, "execute 'any' on 'AbortSignal'");
      const result = new AbortSignal(SIGNAL_KEY);
      const sources = [...signals];
      for (const source of sources) {
        if (!isSignal(source)) {
          throw new TypeError("Failed to execute 'any' on 'AbortSignal': Failed to convert value to 'AbortSignal'.");
        }
      }
      for (const source of sources) {
        const s = signalState(source);
        if (s.aborted) {
          const r = signalState(result);
          r.aborted = true;
          r.reason = s.reason;
          return result;
        }
      }
      for (const source of sources) signalState(source).dependents.push(result);
      return result;
    }
  }
  tag(AbortSignal, 'AbortSignal');
  defineEventHandlers(AbortSignal.prototype, ['onabort']);

  function abortError() {
    return new DOMException('signal is aborted without reason', 'AbortError');
  }

  // Aborts `signal`: runs its abort algorithms, fires `abort`, then aborts
  // signals made with `AbortSignal.any` that follow it.
  function signalAbort(signal, reason) {
    const s = signalState(signal);
    if (s.aborted) return;
    s.aborted = true;
    s.reason = reason === undefined ? abortError() : reason;
    const dependents = s.dependents;
    s.dependents = [];
    const algorithms = [...s.algorithms];
    s.algorithms.clear();
    for (const algorithm of algorithms) {
      try {
        algorithm(s.reason);
      } catch (error) {
        reportError(error);
      }
    }
    fire(signal, 'abort');
    for (const dependent of dependents) signalAbort(dependent, s.reason);
  }

  // Runs `algorithm(reason)` when `signal` aborts, before `abort` listeners.
  // Returns a function that unregisters it.
  function signalAddAlgorithm(signal, algorithm) {
    const s = signalState(signal);
    if (s.aborted) return () => {};
    s.algorithms.add(algorithm);
    return () => s.algorithms.delete(algorithm);
  }

  class AbortController {
    #signal = new AbortSignal(SIGNAL_KEY);
    get signal() {
      return this.#signal;
    }
    abort(reason) {
      signalAbort(this.#signal, reason);
    }
  }
  tag(AbortController, 'AbortController');

  internals.expose({
    DOMException,
    Event,
    CustomEvent,
    ErrorEvent,
    PromiseRejectionEvent,
    EventTarget,
    AbortSignal,
    AbortController,
  });

  Object.assign(internals, {
    queueTask,
    domException,
    eventClass,
    dispatch,
    fire,
    // Called by Rust for each exception that escapes to the event loop.
    reportException,
    // Called by Rust as promise rejections are reported and handled.
    unhandledRejection,
    rejectionOutstanding,
    rejectionHandled,
    defineEventHandlers,
    attributeHandlerChanged,
    existingHandler,
    // The content attribute that sets the handler for a type on a target,
    // as [element, source], or null. The DOM bindings replace it.
    handlerAttribute: () => null,
    // Compiles a content attribute's handler body. The DOM bindings
    // replace it.
    compileHandler: () => null,
    // The "get the parent" hook of event dispatch. The DOM bindings
    // replace it to give nodes their propagation path.
    eventParent: () => null,
    isEvent,
    isSignal,
    signalAbort,
    signalAddAlgorithm,
  });
})
