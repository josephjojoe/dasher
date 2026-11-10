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

  return {
    inspect: (value) => inspect(value, 0, []),
  };
})
