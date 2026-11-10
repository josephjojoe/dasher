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

  globalThis.performance = {
    now() {
      return pageNow();
    },
    get timeOrigin() {
      return timeOrigin;
    },
    toJSON() {
      return { timeOrigin };
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

  return {
    inspect: (value) => inspect(value, 0, []),
    runTimer,
    runFrame,
  };
})
