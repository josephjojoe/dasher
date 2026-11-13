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

  // Event and EventTarget are native (src/events/): their state, listener
  // storage and dispatch live in Rust. https://dom.spec.whatwg.org/#events
  const E = native.events;
  const Event = E.Event;
  const EventTarget = E.EventTarget;
  const isEvent = E.isEvent;

  for (const [name, value] of [['NONE', 0], ['CAPTURING_PHASE', 1], ['AT_TARGET', 2], ['BUBBLING_PHASE', 3]]) {
    defineProperty(Event, name, { value, enumerable: true });
    defineProperty(Event.prototype, name, { value, enumerable: true });
  }
  tag(Event, 'Event');
  tag(EventTarget, 'EventTarget');

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

  // Dispatches a trusted event, and makes and dispatches one if anything
  // listens for it: fire(target, type, init, Ctor = Event).
  const dispatch = E.dispatch;
  const fire = E.fire;

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
  // and, for elements, the `on*` content attributes that set them, kept
  // with the listeners in Rust (see src/events/dispatch.rs).
  // https://html.spec.whatwg.org/#event-handler-attributes
  //
  // Content attributes reach the handlers two ways. Changes made through
  // the DOM call `attributeHandlerChanged` (after `existingHandler`, so an
  // attribute the element was created with keeps its place). Attributes
  // the element was created with (by the parser, innerHTML or cloning) are
  // only looked at when the handler is first needed.

  const defineEventHandlers = E.defineHandlers;

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
      // Listeners added with this signal are removed natively when it
      // aborts.
      E.makeSignal(this);
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
      E.abortSignal(signal);
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
          E.abortSignal(result);
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
    E.abortSignal(signal);
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

  // What dispatch calls back into; the hooks go through `internals` so the
  // DOM bindings can replace them.
  E.setHooks({
    compileHandler: (el, type, source, forWindow) => internals.compileHandler(el, type, source, forWindow),
    slotFor: (node) => internals.slotFor(node),
    domException: (name, message) => new DOMException(message, name),
    ErrorEvent,
  });

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
    attributeHandlerChanged: E.attributeHandlerChanged,
    existingHandler: E.existingHandler,
    // Which event types have handler content attributes: those the body
    // sets the window's handlers with, and elements' own.
    handlerTypes: E.handlerTypes,
    // A native EventTarget with the given prototype, for objects made
    // without a constructor (Attr).
    createTarget: E.createTarget,
    // Compiles a content attribute's handler body. The DOM bindings
    // replace it.
    compileHandler: () => null,
    // The slot a node's event path goes through. The DOM bindings replace
    // it.
    slotFor: () => null,
    isEvent,
    isSignal,
    signalAbort,
    signalAddAlgorithm,
  });
})
