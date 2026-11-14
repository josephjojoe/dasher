// Runtime glue evaluated once per context, before any page script.
//
// This file evaluates to a function that receives `native`, an object of
// host functions implemented in Rust, installs the page-visible globals and
// returns the hooks Rust calls back into. `native` and the hooks stay in this
// closure, so page scripts can neither reach nor replace them.
(function (native) {
  'use strict';

  const ObjectKeys = Object.keys;
  const ArrayIsArray = Array.isArray;
  const JSONStringify = JSON.stringify;
  const ErrorCtor = Error;
  const ReflectApply = Reflect.apply;
  const ReflectConstruct = Reflect.construct;
  const PromiseThen = Promise.prototype.then;
  const resolvedPromise = Promise.resolve();
  const indirectEval = eval;

  const MAX_DEPTH = 2;
  const MAX_ITEMS = 100;

  // Renders a value roughly the way browser consoles do. Never throws.
  function inspect(value, depth, seen) {
    switch (typeof value) {
      case 'string':
        return depth === 0 ? value : JSONStringify(value);
      case 'number':
        return Object.is(value, -0) ? '-0' : String(value);
      case 'bigint':
        return String(value) + 'n';
      case 'boolean':
      case 'undefined':
        return String(value);
      case 'symbol':
        return value.toString();
      case 'function': {
        const name = value.name ? value.name : '(anonymous)';
        return /^class\b/.test(Function.prototype.toString.call(value))
          ? '[class ' + name + ']'
          : '[Function: ' + name + ']';
      }
    }
    if (value === null) return 'null';
    try {
      return inspectObject(value, depth, seen);
    } catch (_) {
      return '[object]';
    }
  }

  function inspectObject(value, depth, seen) {
    if (value instanceof ErrorCtor) {
      const head = String(value);
      const stack = typeof value.stack === 'string' ? value.stack.trimEnd() : '';
      return stack ? head + '\n' + stack : head;
    }
    if (value instanceof Date) {
      return isNaN(value.getTime()) ? 'Invalid Date' : value.toISOString();
    }
    if (value instanceof RegExp) return String(value);
    if (value instanceof Promise) return 'Promise {}';
    if (seen.includes(value)) return '[Circular]';
    const ctor = constructorName(value);
    if (depth > MAX_DEPTH) {
      return ArrayIsArray(value) ? '[Array]' : '[' + (ctor || 'Object') + ']';
    }
    seen.push(value);
    try {
      if (ArrayIsArray(value)) {
        return '[' + list(value, value.length, (v) => inspect(v, depth + 1, seen)) + ']';
      }
      if (value instanceof Map) {
        const items = list([...value], value.size, ([k, v]) =>
          inspect(k, depth + 1, seen) + ' => ' + inspect(v, depth + 1, seen));
        return 'Map(' + value.size + ') {' + items + '}';
      }
      if (value instanceof Set) {
        const items = list([...value], value.size, (v) => inspect(v, depth + 1, seen));
        return 'Set(' + value.size + ') {' + items + '}';
      }
      const keys = ObjectKeys(value);
      const items = list(keys, keys.length, (k) => {
        let v;
        try {
          v = value[k];
        } catch (_) {
          v = '[Getter threw]';
        }
        return propertyKey(k) + ': ' + inspect(v, depth + 1, seen);
      });
      const prefix = ctor && ctor !== 'Object' ? ctor + ' ' : '';
      return prefix + '{' + items + '}';
    } finally {
      seen.pop();
    }
  }

  function list(items, length, render) {
    const shown = items.slice(0, MAX_ITEMS).map(render);
    if (length > MAX_ITEMS) shown.push('... ' + (length - MAX_ITEMS) + ' more items');
    return shown.join(', ');
  }

  function constructorName(value) {
    const proto = Object.getPrototypeOf(value);
    if (proto === null) return '[Object: null prototype]';
    const ctor = proto.constructor;
    return typeof ctor === 'function' && typeof ctor.name === 'string' ? ctor.name : '';
  }

  function propertyKey(key) {
    return /^[A-Za-z_$][\w$]*$/.test(key) ? key : JSONStringify(key);
  }

  // Renders console arguments, applying format specifiers in a leading
  // string. https://console.spec.whatwg.org/#formatter
  function format(args) {
    if (args.length === 0) return '';
    try {
      const first = args[0];
      let next = 1;
      let out;
      if (typeof first === 'string' && first.includes('%')) {
        out = first.replace(/%([sdifoOjc%])/g, (spec, c) => {
          if (c === '%') return '%';
          if (next >= args.length) return spec;
          const arg = args[next++];
          switch (c) {
            case 's':
              return typeof arg === 'string' ? arg : inspect(arg, 1, []);
            case 'd':
            case 'i':
              return typeof arg === 'symbol' ? 'NaN' : String(Math.trunc(Number(arg)));
            case 'f':
              return typeof arg === 'symbol' ? 'NaN' : String(Number(arg));
            case 'c':
              return '';
            default:
              return inspect(arg, 1, []);
          }
        });
      } else {
        out = inspect(first, 0, []);
      }
      for (; next < args.length; next++) out += ' ' + inspect(args[next], 0, []);
      return out;
    } catch (_) {
      return '[unformattable]';
    }
  }

  // https://html.spec.whatwg.org/#dom-reporterror
  globalThis.reportError = function reportError(error) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'reportError': 1 argument required, but only 0 present.");
    }
    native.reportError(error);
  };

  // Timers. Rust keeps ids and due times; the callbacks stay here.
  // https://html.spec.whatwg.org/#timers

  const timerCallbacks = new Map();

  function addTimer(handler, timeout, args, repeat) {
    // WebIDL `long`: ToInt32, so huge timeouts wrap like in browsers.
    let ms = timeout | 0;
    if (ms < 0) ms = 0;
    const id = native.setTimer(ms, repeat);
    timerCallbacks.set(id, { handler, args });
    return id;
  }

  // A timer for requestIdleCallback: it runs when due, but does not keep
  // the page busy (see timers.rs).
  function setIdleTimer(handler, timeout) {
    const id = native.setIdleTimer(timeout);
    timerCallbacks.set(id, { handler, args: [] });
    return id;
  }

  function clearTimer(id) {
    const n = id >>> 0;
    if (n !== 0 && timerCallbacks.delete(n)) native.clearTimer(n);
  }

  globalThis.setTimeout = function setTimeout(handler, timeout = 0, ...args) {
    return addTimer(handler, timeout, args, false);
  };
  globalThis.setInterval = function setInterval(handler, timeout = 0, ...args) {
    return addTimer(handler, timeout, args, true);
  };
  globalThis.clearTimeout = function clearTimeout(id) {
    clearTimer(id);
  };
  globalThis.clearInterval = function clearInterval(id) {
    clearTimer(id);
  };

  function runTimer(id, repeat) {
    const entry = timerCallbacks.get(id);
    if (entry === undefined) return;
    if (!repeat) timerCallbacks.delete(id);
    const handler = entry.handler;
    if (typeof handler === 'function') {
      ReflectApply(handler, globalThis, entry.args);
    } else {
      indirectEval(String(handler));
    }
  }

  // Animation frames, driven by the virtual frame clock in Rust.

  const frameCallbacks = new Map();

  globalThis.requestAnimationFrame = function requestAnimationFrame(callback) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'requestAnimationFrame': parameter 1 is not of type 'Function'.");
    }
    const id = native.requestFrame();
    frameCallbacks.set(id, callback);
    return id;
  };
  globalThis.cancelAnimationFrame = function cancelAnimationFrame(id) {
    const n = id >>> 0;
    if (frameCallbacks.delete(n)) native.cancelFrame(n);
  };

  function runFrame(id, timestamp) {
    const callback = frameCallbacks.get(id);
    if (callback === undefined) return;
    frameCallbacks.delete(id);
    ReflectApply(callback, globalThis, [timestamp]);
  }

  // Microtasks share QuickJS's promise job queue, so they interleave with
  // promise reactions in order. Exceptions are reported, not rejected.
  globalThis.queueMicrotask = function queueMicrotask(callback) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'queueMicrotask': parameter 1 is not of type 'Function'.");
    }
    ReflectApply(PromiseThen, resolvedPromise, [() => {
      try {
        callback();
      } catch (error) {
        native.reportError(error);
      }
    }]);
  };

  // Page time. `performance.now()` and `Date` both follow the page clock, so
  // when the event loop fast-forwards, scripts that measure elapsed time
  // with either see the time pass.

  const timeOrigin = native.timeOrigin;
  const pageNow = native.now;
  const empty = () => [];
  const noop = () => undefined;

  // Navigation Timing Level 1, which pages still read for their own load
  // metrics: https://www.w3.org/TR/navigation-timing/
  //
  // The document's stages are timed (native.loader.milestones). The network
  // is not: every stage up to the response reports the navigation start, and
  // nothing was unloaded or redirected.

  const TIMING_KEY = {};
  const navigationStart = Math.floor(timeOrigin);
  const milestone = (index) => {
    const at = native.loader.milestones()[index];
    return at < 0 ? 0 : Math.floor(timeOrigin + at);
  };
  const TIMING = {
    navigationStart: () => navigationStart,
    unloadEventStart: () => 0,
    unloadEventEnd: () => 0,
    redirectStart: () => 0,
    redirectEnd: () => 0,
    fetchStart: () => navigationStart,
    domainLookupStart: () => navigationStart,
    domainLookupEnd: () => navigationStart,
    connectStart: () => navigationStart,
    connectEnd: () => navigationStart,
    secureConnectionStart: () => (globalThis.location?.protocol === 'https:' ? navigationStart : 0),
    requestStart: () => navigationStart,
    responseStart: () => navigationStart,
    responseEnd: () => navigationStart,
    domLoading: () => navigationStart,
    domInteractive: () => milestone(0),
    domContentLoadedEventStart: () => milestone(1),
    domContentLoadedEventEnd: () => milestone(2),
    domComplete: () => milestone(3),
    loadEventStart: () => milestone(4),
    loadEventEnd: () => milestone(5),
  };
  const timingInterface = (name, attributes, constants) => {
    const ctor = {
      [name]: function (key) {
        if (key !== TIMING_KEY) throw new TypeError('Illegal constructor');
      },
    }[name];
    const proto = ctor.prototype;
    for (const key of Object.keys(attributes)) {
      Object.defineProperty(proto, key, { get: attributes[key], enumerable: true, configurable: true });
    }
    Object.defineProperty(proto, 'toJSON', {
      value: function toJSON() {
        const json = {};
        for (const key of Object.keys(attributes)) json[key] = this[key];
        return json;
      },
      writable: true,
      enumerable: true,
      configurable: true,
    });
    for (const key of Object.keys(constants)) {
      const constant = { value: constants[key], enumerable: true };
      Object.defineProperty(ctor, key, constant);
      Object.defineProperty(proto, key, constant);
    }
    Object.defineProperty(proto, Symbol.toStringTag, { value: name, configurable: true });
    Object.defineProperty(globalThis, name, { value: ctor, writable: true, configurable: true });
    return new ctor(TIMING_KEY);
  };
  const timing = timingInterface('PerformanceTiming', TIMING, {});
  const navigation = timingInterface(
    'PerformanceNavigation',
    { type: () => 0, redirectCount: () => 0 },
    { TYPE_NAVIGATE: 0, TYPE_RELOAD: 1, TYPE_BACK_FORWARD: 2, TYPE_RESERVED: 255 },
  );

  globalThis.performance = {
    now() {
      return pageNow();
    },
    get timeOrigin() {
      return timeOrigin;
    },
    get timing() {
      return timing;
    },
    get navigation() {
      return navigation;
    },
    toJSON() {
      return { timeOrigin, timing: timing.toJSON(), navigation: navigation.toJSON() };
    },
    mark: noop,
    measure: noop,
    clearMarks: noop,
    clearMeasures: noop,
    clearResourceTimings: noop,
    getEntries: empty,
    getEntriesByName: empty,
    getEntriesByType: empty,
  };

  const NativeDate = Date;
  const dateNow = () => Math.floor(timeOrigin + pageNow());
  const PageDate = new Proxy(NativeDate, {
    construct(target, args, newTarget) {
      return ReflectConstruct(target, args.length === 0 ? [dateNow()] : args, newTarget);
    },
    apply() {
      return new NativeDate(dateNow()).toString();
    },
  });
  NativeDate.now = function now() {
    return dateNow();
  };
  NativeDate.prototype.constructor = PageDate;
  globalThis.Date = PageDate;

  // Console output is captured into a log the host reads.
  // https://console.spec.whatwg.org/

  let groupIndent = '';
  const counts = new Map();
  const consoleTimers = new Map();

  function emit(level, text) {
    if (groupIndent) text = groupIndent + text.split('\n').join('\n' + groupIndent);
    native.log(level, text);
  }

  function label(value) {
    return value === undefined ? 'default' : String(value);
  }

  function timeLog(name, data, end) {
    const start = consoleTimers.get(name);
    if (start === undefined) {
      emit('warn', "Timer '" + name + "' does not exist");
      return;
    }
    if (end) consoleTimers.delete(name);
    const extra = data.length ? ' ' + format(data) : '';
    emit('log', name + ': ' + (pageNow() - start) + ' ms' + extra);
  }

  function group(args) {
    if (args.length) emit('log', format(args));
    groupIndent += '  ';
  }

  globalThis.console = {
    log(...args) { emit('log', format(args)); },
    info(...args) { emit('info', format(args)); },
    warn(...args) { emit('warn', format(args)); },
    error(...args) { emit('error', format(args)); },
    debug(...args) { emit('debug', format(args)); },
    dir(item) { emit('log', inspect(item, 1, [])); },
    dirxml(...args) { emit('log', format(args)); },
    table(...args) { emit('log', format(args)); },
    trace(...args) {
      const stack = String(new ErrorCtor().stack || '')
        .split('\n')
        .filter((line) => line.trim() && !line.includes('dasher:prelude'))
        .join('\n');
      const head = args.length ? 'Trace: ' + format(args) : 'Trace';
      emit('log', stack ? head + '\n' + stack : head);
    },
    assert(condition, ...args) {
      if (condition) return;
      emit('error', args.length ? 'Assertion failed: ' + format(args) : 'Assertion failed');
    },
    count(name) {
      name = label(name);
      const n = (counts.get(name) || 0) + 1;
      counts.set(name, n);
      emit('log', name + ': ' + n);
    },
    countReset(name) { counts.delete(label(name)); },
    time(name) { consoleTimers.set(label(name), pageNow()); },
    timeLog(name, ...data) { timeLog(label(name), data, false); },
    timeEnd(name) { timeLog(label(name), [], true); },
    group(...args) { group(args); },
    groupCollapsed(...args) { group(args); },
    groupEnd() { groupIndent = groupIndent.slice(2); },
    clear() {},
  };

  // Converts a value to a USVString: lone surrogates become U+FFFD. Strings
  // must go through this before reaching Rust, which only takes valid UTF-8.
  function usv(value) {
    return String(value).toWellFormed();
  }

  // Installs interfaces and functions on the global object the way WebIDL
  // does: writable and configurable, but not enumerable.
  function expose(members) {
    for (const name of Object.keys(members)) {
      Object.defineProperty(globalThis, name, {
        value: members[name],
        writable: true,
        configurable: true,
        enumerable: false,
      });
    }
  }

  // The `internals` object: hooks Rust calls back into, plus helpers shared
  // with the glue files evaluated after this one.
  return {
    inspect: (value) => inspect(value, 0, []),
    runTimer,
    setIdleTimer,
    runFrame,
    usv,
    expose,
  };
})
