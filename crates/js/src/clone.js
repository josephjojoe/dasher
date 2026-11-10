// structuredClone, also used for history state and postMessage.
// https://html.spec.whatwg.org/#structured-cloning
//
// Supports what bundles actually put through it: primitives, plain objects
// and arrays (including cycles and shared references), Date, RegExp, Map,
// Set, ArrayBuffer, typed arrays, DataView, primitive wrappers, errors and
// DOMException. Functions, symbols, weak collections, promises and other
// platform objects throw a DataCloneError. Transferred ArrayBuffers are
// detached in the source.
(function (native, internals) {
  'use strict';

  const domException = internals.domException;
  const ObjectKeys = Object.keys;
  const defineProperty = Object.defineProperty;
  const isView = ArrayBuffer.isView;
  const typedArrayTag = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const arrayBufferSlice = ArrayBuffer.prototype.slice;
  const arrayBufferTransfer = ArrayBuffer.prototype.transfer;
  const NativeDate = Object.getPrototypeOf(new Date(0)).constructor;
  const dateGetTime = NativeDate.prototype.getTime;

  const typedArrays = {
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array,
    Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array,
  };
  if (typeof Float16Array === 'function') typedArrays.Float16Array = Float16Array;

  const errorCtors = {
    Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError,
  };

  // Values that hold internal state the clone cannot reproduce.
  const uncloneable = [WeakMap, WeakSet, Promise];
  if (typeof WeakRef === 'function') uncloneable.push(WeakRef);
  if (typeof FinalizationRegistry === 'function') uncloneable.push(FinalizationRegistry);

  function cannotClone(value) {
    let shown;
    try {
      shown = typeof value === 'function' ? String(value) : Object.prototype.toString.call(value);
    } catch (_) {
      shown = 'value';
    }
    if (shown.length > 60) shown = shown.slice(0, 57) + '...';
    return domException('DataCloneError',
      "Failed to execute 'structuredClone' on 'Window': " + shown + ' could not be cloned.');
  }

  function isPlatformObject(value) {
    return (typeof EventTarget === 'function' && value instanceof EventTarget) ||
      (typeof Event === 'function' && value instanceof Event) ||
      (typeof URL === 'function' && value instanceof URL) ||
      (typeof URLSearchParams === 'function' && value instanceof URLSearchParams);
  }

  function setOwn(target, key, value) {
    defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });
  }

  function copyProperties(source, target, memory) {
    for (const key of ObjectKeys(source)) setOwn(target, key, clone(source[key], memory));
  }

  function wrapperValue(value) {
    // Brand checks through the prototype methods: they throw on anything
    // that lacks the internal slot.
    for (const [Ctor, valueOf] of [
      [Boolean, Boolean.prototype.valueOf],
      [Number, Number.prototype.valueOf],
      [String, String.prototype.valueOf],
      [BigInt, BigInt.prototype.valueOf],
    ]) {
      try {
        return { found: true, value: Object(valueOf.call(value)), Ctor };
      } catch (_) {
        // Not this kind of wrapper.
      }
    }
    return { found: false };
  }

  function clone(value, memory) {
    const type = typeof value;
    if (type === 'symbol' || type === 'function') throw cannotClone(value);
    if (type !== 'object' || value === null) return value;
    const seen = memory.get(value);
    if (seen !== undefined) return seen;

    let out;
    if (Array.isArray(value)) {
      out = new Array(value.length);
      memory.set(value, out);
      copyProperties(value, out, memory);
      return out;
    }
    if (value instanceof NativeDate) {
      out = new NativeDate(dateGetTime.call(value));
    } else if (value instanceof RegExp) {
      out = new RegExp(value.source, value.flags);
    } else if (value instanceof ArrayBuffer) {
      out = arrayBufferSlice.call(value, 0);
    } else if (isView(value)) {
      const buffer = clone(value.buffer, memory);
      if (value instanceof DataView) {
        out = new DataView(buffer, value.byteOffset, value.byteLength);
      } else {
        const Ctor = typedArrays[typedArrayTag.call(value)];
        out = new Ctor(buffer, value.byteOffset, value.length);
      }
    } else if (value instanceof Map) {
      out = new Map();
      memory.set(value, out);
      for (const [k, v] of [...value]) out.set(clone(k, memory), clone(v, memory));
      return out;
    } else if (value instanceof Set) {
      out = new Set();
      memory.set(value, out);
      for (const v of [...value]) out.add(clone(v, memory));
      return out;
    } else if (typeof DOMException === 'function' && value instanceof DOMException) {
      out = new DOMException(value.message, value.name);
    } else if (value instanceof Error) {
      const name = value.name;
      const Ctor = Object.prototype.hasOwnProperty.call(errorCtors, name) ? errorCtors[name] : Error;
      const message = Object.getOwnPropertyDescriptor(value, 'message');
      out = message === undefined ? new Ctor() : new Ctor(String(message.value));
      if (typeof value.stack === 'string') {
        defineProperty(out, 'stack', { value: value.stack, writable: true, configurable: true });
      }
    } else if (isPlatformObject(value) || uncloneable.some((Ctor) => value instanceof Ctor)) {
      throw cannotClone(value);
    } else {
      const wrapper = wrapperValue(value);
      if (wrapper.found) {
        out = wrapper.value;
      } else {
        // Ordinary objects, including class instances, become plain objects.
        out = {};
        memory.set(value, out);
        copyProperties(value, out, memory);
        return out;
      }
    }
    memory.set(value, out);
    return out;
  }

  function structuredClone(value, options) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'structuredClone' on 'Window': 1 argument required, but only 0 present.");
    }
    const memory = new Map();
    const transfer = options === undefined || options === null || options.transfer === undefined
      ? [] : [...options.transfer];
    for (const buffer of transfer) {
      if (!(buffer instanceof ArrayBuffer)) {
        throw domException('DataCloneError', "Failed to execute 'structuredClone' on 'Window': Value at index 0 does not have a transferable type.");
      }
      if (memory.has(buffer)) {
        throw domException('DataCloneError', "Failed to execute 'structuredClone' on 'Window': ArrayBuffer at index 1 is a duplicate of an earlier ArrayBuffer.");
      }
      memory.set(buffer, arrayBufferSlice.call(buffer, 0));
    }
    const result = clone(value, memory);
    // Transferring moves the bytes: the source buffers end up detached.
    for (const buffer of transfer) arrayBufferTransfer.call(buffer);
    return result;
  }

  internals.expose({ structuredClone });
  internals.structuredClone = (value) => clone(value, new Map());
})
