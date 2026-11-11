// Globals whose glue is compiled with the rest but only evaluated the first
// time a page uses them, so pages that never do don't carry them in their
// heap. A lazy glue file evaluates to a function taking `native` and
// `internals` that returns what it implements without installing anything:
// this file owns the globals and methods, so a page that replaced one
// before first use keeps its replacement.
(function (native, internals) {
  'use strict';

  const defineProperty = Object.defineProperty;
  const ReflectApply = Reflect.apply;

  // Streams (streams.js). Reading one of the stream globals, or a body as
  // a stream (fetch.js, through `internals.streams`), loads them.
  let streams;
  const loadStreams = () => streams ??= native.loadGlue('dasher:streams')(native, internals);
  internals.streams = loadStreams;
  // Nothing is a stream until streams are loaded.
  internals.isReadableStream = (value) => streams !== undefined && streams.isReadable(value);
  for (const name of ['ReadableStream', 'ReadableStreamDefaultReader', 'ReadableStreamDefaultController',
    'ReadableByteStreamController', 'WritableStream', 'WritableStreamDefaultWriter',
    'WritableStreamDefaultController', 'TransformStream', 'TransformStreamDefaultController',
    'ByteLengthQueuingStrategy', 'CountQueuingStrategy', 'TextEncoderStream', 'TextDecoderStream']) {
    const set = (value) => {
      defineProperty(globalThis, name, { value, writable: true, configurable: true });
    };
    defineProperty(globalThis, name, {
      get() {
        const value = loadStreams().globals[name];
        set(value);
        return value;
      },
      set,
      configurable: true,
    });
  }

  // Intl, and the built-in methods ECMA-402 makes locale-sensitive
  // (intl.js). Reading `Intl` or calling one of the methods loads it.
  let intl;
  const loadIntl = () => intl ??= native.loadGlue('dasher:intl')(native, internals);
  const setIntl = (value) => {
    defineProperty(globalThis, 'Intl', { value, writable: true, configurable: true });
  };
  defineProperty(globalThis, 'Intl', {
    get() {
      const value = loadIntl().Intl;
      setIntl(value);
      return value;
    },
    set: setIntl,
    configurable: true,
  });
  const methods = [[Number, 'toLocaleString'], [BigInt, 'toLocaleString'], [Date, 'toLocaleString'],
    [Date, 'toLocaleDateString'], [Date, 'toLocaleTimeString'], [String, 'localeCompare']];
  for (const [ctor, name] of methods) {
    const key = ctor.name;
    const method = {
      [name]() {
        return ReflectApply(loadIntl()[key][name], this, arguments);
      },
    }[name];
    defineProperty(ctor.prototype, name, { value: method, writable: true, configurable: true });
  }
})
