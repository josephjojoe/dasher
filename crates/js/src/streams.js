// Streams: ReadableStream, WritableStream, TransformStream, their readers,
// writers and controllers, the queuing strategies, and TextEncoderStream /
// TextDecoderStream.
// https://streams.spec.whatwg.org/
// https://encoding.spec.whatwg.org/#interface-mixin-generictransformstream
//
// Pages build these themselves (Next.js's App Router feeds its inline
// flight data to React through a ReadableStream) and get them from
// `response.body` and `blob.stream()`. The algorithms follow the spec for
// default (non-byte) streams. Not supported: byte streams' BYOB readers
// (`type: 'bytes'` streams work as default streams of Uint8Array chunks,
// and `getReader({ mode: 'byob' })` throws), and transferring streams.
(function (native, internals) {
  'use strict';

  const PromiseRef = Promise;
  const PromiseThen = Promise.prototype.then;
  const PromiseCatch = Promise.prototype.catch;
  const ReflectApply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const AbortControllerRef = globalThis.AbortController;
  const TextEncoderRef = globalThis.TextEncoder;
  const TextDecoderRef = globalThis.TextDecoder;
  const noop = () => {};
  const queueMicrotask = globalThis.queueMicrotask;
  const KEY = Symbol('streams');

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  function illegal(key) {
    if (key !== KEY) throw new TypeError('Illegal constructor');
  }

  function then(promise, onFulfilled, onRejected) {
    return ReflectApply(PromiseThen, promise, [onFulfilled, onRejected]);
  }

  function handled(promise) {
    ReflectApply(PromiseCatch, promise, [noop]);
    return promise;
  }

  // A promise with its settle functions, and a `settled` flag.
  function deferred() {
    const d = { promise: null, resolve: null, reject: null, settled: false };
    d.promise = new PromiseRef((resolve, reject) => {
      d.resolve = (v) => {
        d.settled = true;
        resolve(v);
      };
      d.reject = (e) => {
        d.settled = true;
        reject(e);
      };
    });
    return d;
  }

  function resolvedWith(value) {
    return PromiseRef.resolve(value);
  }

  function rejectedWith(error) {
    return handled(PromiseRef.reject(error));
  }

  // Calls `fn` and returns a promise of its result, or a rejected promise
  // if it throws.
  function promiseCall(fn, thisArg, args) {
    try {
      return resolvedWith(ReflectApply(fn, thisArg, args));
    } catch (error) {
      return rejectedWith(error);
    }
  }

  // An underlying source or sink method as a function of `args`, or one
  // returning undefined if the method is absent.
  function method(obj, name, what) {
    if (obj === undefined || obj === null) return noop;
    const fn = obj[name];
    if (fn === undefined || fn === null) return noop;
    if (typeof fn !== 'function') {
      throw new TypeError('Failed to construct \'' + what + '\': ' + name + ' is not a function.');
    }
    return fn;
  }

  function dictionary(value, what) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError('Failed to ' + what + ": The provided value is not of type 'Dictionary'.");
    }
    return value;
  }

  function highWaterMark(strategy, fallback, what) {
    const hwm = strategy.highWaterMark;
    if (hwm === undefined) return fallback;
    const n = Number(hwm);
    if (Number.isNaN(n) || n < 0) {
      throw new RangeError('Failed to construct \'' + what + '\': Cannot convert to a valid high water mark.');
    }
    return n;
  }

  function sizeAlgorithm(strategy) {
    const size = strategy.size;
    if (size === undefined) return () => 1;
    if (typeof size !== 'function') throw new TypeError('size is not a function.');
    return (chunk) => size(chunk);
  }

  // A queue with sizes, as in "queue-with-sizes".
  function enqueueValue(c, value, size) {
    if (typeof size !== 'number' || Number.isNaN(size) || size < 0 || size === Infinity) {
      throw new RangeError('The return value of a queuing strategy\'s size function must be a finite, non-NaN, non-negative number.');
    }
    c.queue.push({ value, size });
    c.queueTotalSize += size;
  }

  function dequeueValue(c) {
    const pair = c.queue.shift();
    c.queueTotalSize -= pair.size;
    if (c.queueTotalSize < 0) c.queueTotalSize = 0;
    return pair.value;
  }

  function resetQueue(c) {
    c.queue = [];
    c.queueTotalSize = 0;
  }

  // ---------------------------------------------------------------------
  // ReadableStream.

  let readableState;
  let isReadable;

  class ReadableStream {
    #s;
    constructor(underlyingSource = undefined, strategy = undefined) {
      const what = 'ReadableStream';
      const source = underlyingSource === null ? undefined : underlyingSource;
      if (source !== undefined && typeof source !== 'object' && typeof source !== 'function') {
        throw new TypeError("Failed to construct 'ReadableStream': The provided value is not of type 'UnderlyingSource'.");
      }
      const strat = dictionary(strategy, "construct 'ReadableStream'");
      this.#s = newReadableState();
      const type = source === undefined ? undefined : source.type;
      if (type !== undefined && String(type) !== 'bytes') {
        throw new TypeError("Failed to construct 'ReadableStream': The provided value '" + type + "' is not a valid enum value of type ReadableStreamType.");
      }
      const isBytes = type !== undefined;
      if (isBytes && strat.size !== undefined) {
        throw new RangeError("Failed to construct 'ReadableStream': The strategy for a byte stream cannot have a size function.");
      }
      const hwm = highWaterMark(strat, isBytes ? 0 : 1, what);
      const size = isBytes ? (chunk) => chunkByteSize(chunk) : sizeAlgorithm(strat);
      const start = method(source, 'start', what);
      const pull = method(source, 'pull', what);
      const cancel = method(source, 'cancel', what);
      setUpController(this, {
        start: (c) => ReflectApply(start, source, [c]),
        pull: (c) => promiseCall(pull, source, [c]),
        cancel: (reason) => promiseCall(cancel, source, [reason]),
      }, hwm, size, isBytes);
    }
    static {
      readableState = (stream) => stream.#s;
      isReadable = (value) => value !== null && typeof value === 'object' && #s in value;
    }
    get locked() {
      return locked(check(this));
    }
    cancel(reason = undefined) {
      if (!isReadable(this)) return rejectedWith(new TypeError('Illegal invocation'));
      const s = readableState(this);
      if (locked(s)) {
        return rejectedWith(new TypeError("Failed to execute 'cancel' on 'ReadableStream': Cannot cancel a locked stream"));
      }
      return cancelStream(s, reason);
    }
    getReader(options = undefined) {
      check(this);
      const mode = dictionary(options, "execute 'getReader' on 'ReadableStream'").mode;
      if (mode !== undefined) {
        if (String(mode) === 'byob') {
          throw new TypeError("Failed to execute 'getReader' on 'ReadableStream': BYOB readers are not supported.");
        }
        throw new TypeError("Failed to execute 'getReader' on 'ReadableStream': The provided value '" + mode + "' is not a valid enum value of type ReadableStreamReaderMode.");
      }
      return new ReadableStreamDefaultReader(this);
    }
    pipeThrough(transform, options = undefined) {
      const s = check(this);
      const pair = dictionary(transform, "execute 'pipeThrough' on 'ReadableStream'");
      const { readable, writable } = pair;
      if (!isReadable(readable)) {
        throw new TypeError("Failed to execute 'pipeThrough' on 'ReadableStream': Failed to read the 'readable' property from 'ReadableWritablePair': required member readable is undefined.");
      }
      if (!isWritable(writable)) {
        throw new TypeError("Failed to execute 'pipeThrough' on 'ReadableStream': Failed to read the 'writable' property from 'ReadableWritablePair': required member writable is undefined.");
      }
      if (locked(s)) {
        throw new TypeError("Failed to execute 'pipeThrough' on 'ReadableStream': Cannot pipe a locked stream");
      }
      if (writableState(writable).writer !== null) {
        throw new TypeError("Failed to execute 'pipeThrough' on 'ReadableStream': Cannot pipe to a locked stream");
      }
      handled(pipe(this, writable, pipeOptions(options, 'pipeThrough')));
      return readable;
    }
    pipeTo(destination, options = undefined) {
      if (!isReadable(this)) return rejectedWith(new TypeError('Illegal invocation'));
      if (!isWritable(destination)) {
        return rejectedWith(new TypeError("Failed to execute 'pipeTo' on 'ReadableStream': parameter 1 is not of type 'WritableStream'."));
      }
      let opts;
      try {
        opts = pipeOptions(options, 'pipeTo');
      } catch (error) {
        return rejectedWith(error);
      }
      if (locked(readableState(this))) {
        return rejectedWith(new TypeError("Failed to execute 'pipeTo' on 'ReadableStream': Cannot pipe a locked stream"));
      }
      if (writableState(destination).writer !== null) {
        return rejectedWith(new TypeError("Failed to execute 'pipeTo' on 'ReadableStream': Cannot pipe to a locked stream"));
      }
      return pipe(this, destination, opts);
    }
    tee() {
      check(this);
      return tee(this);
    }
    values(options = undefined) {
      check(this);
      const preventCancel = !!dictionary(options, "execute 'values' on 'ReadableStream'").preventCancel;
      return iterate(this, preventCancel);
    }
    [Symbol.asyncIterator](options = undefined) {
      return this.values(options);
    }
    static from(asyncIterable) {
      return readableFrom(asyncIterable);
    }
  }
  tag(ReadableStream, 'ReadableStream');

  function check(stream) {
    if (!isReadable(stream)) throw new TypeError('Illegal invocation');
    return readableState(stream);
  }

  function newReadableState() {
    return {
      state: 'readable',
      reader: null,
      storedError: undefined,
      disturbed: false,
      controller: null,
    };
  }

  function locked(s) {
    return s.reader !== null;
  }

  function chunkByteSize(chunk) {
    if (!ArrayBuffer.isView(chunk)) {
      throw new TypeError("Failed to execute 'enqueue' on 'ReadableByteStreamController': parameter 1 is not of type 'ArrayBufferView'.");
    }
    if (chunk.byteLength === 0) {
      throw new TypeError("Failed to execute 'enqueue' on 'ReadableByteStreamController': chunk must have a non-zero byteLength.");
    }
    return chunk.byteLength;
  }

  // ReadableStreamDefaultController. Byte streams get the same controller
  // under the ReadableByteStreamController name, with `byobRequest` null.
  let controllerState;

  class ReadableStreamDefaultController {
    #c;
    constructor(key, c) {
      illegal(key);
      this.#c = c;
    }
    static {
      controllerState = (controller) => controller.#c;
    }
    get desiredSize() {
      return desiredSize(this.#c);
    }
    close() {
      const c = this.#c;
      if (!canCloseOrEnqueue(c)) {
        throw new TypeError("Failed to execute 'close' on 'ReadableStreamDefaultController': The stream is not in a state that permits close");
      }
      closeController(c);
    }
    enqueue(chunk = undefined) {
      const c = this.#c;
      if (!canCloseOrEnqueue(c)) {
        throw new TypeError("Failed to execute 'enqueue' on 'ReadableStreamDefaultController': The stream is not in a state that permits enqueue");
      }
      enqueueChunk(c, chunk);
    }
    error(e = undefined) {
      errorController(this.#c, e);
    }
  }
  tag(ReadableStreamDefaultController, 'ReadableStreamDefaultController');

  class ReadableByteStreamController extends ReadableStreamDefaultController {
    get byobRequest() {
      return null;
    }
  }
  tag(ReadableByteStreamController, 'ReadableByteStreamController');

  function setUpController(stream, algorithms, hwm, size, isBytes) {
    const s = readableState(stream);
    const c = {
      stream: s,
      queue: [],
      queueTotalSize: 0,
      started: false,
      closeRequested: false,
      pullAgain: false,
      pulling: false,
      hwm,
      size,
      pull: algorithms.pull,
      cancel: algorithms.cancel,
      object: null,
    };
    const Controller = isBytes ? ReadableByteStreamController : ReadableStreamDefaultController;
    c.object = new Controller(KEY, c);
    s.controller = c;
    let startResult;
    try {
      startResult = algorithms.start(c.object);
    } catch (error) {
      errorController(c, error);
      c.started = true;
      return;
    }
    then(resolvedWith(startResult), () => {
      c.started = true;
      pullIfNeeded(c);
    }, (error) => errorController(c, error));
  }

  function desiredSize(c) {
    const state = c.stream.state;
    if (state === 'errored') return null;
    if (state === 'closed') return 0;
    return c.hwm - c.queueTotalSize;
  }

  function canCloseOrEnqueue(c) {
    return !c.closeRequested && c.stream.state === 'readable';
  }

  function clearAlgorithms(c) {
    c.pull = null;
    c.cancel = null;
    c.size = null;
  }

  function closeController(c) {
    if (!canCloseOrEnqueue(c)) return;
    c.closeRequested = true;
    if (c.queue.length === 0) {
      clearAlgorithms(c);
      closeStream(c.stream);
    }
  }

  function enqueueChunk(c, chunk) {
    if (!canCloseOrEnqueue(c)) return;
    const s = c.stream;
    if (s.reader !== null && s.reader.readRequests.length > 0) {
      s.reader.readRequests.shift().chunk(chunk);
    } else {
      try {
        enqueueValue(c, chunk, c.size(chunk));
      } catch (error) {
        errorController(c, error);
        throw error;
      }
    }
    pullIfNeeded(c);
  }

  function errorController(c, e) {
    if (c.stream.state !== 'readable') return;
    resetQueue(c);
    clearAlgorithms(c);
    errorStream(c.stream, e);
  }

  function shouldPull(c) {
    if (!canCloseOrEnqueue(c) || !c.started) return false;
    const s = c.stream;
    if (s.reader !== null && s.reader.readRequests.length > 0) return true;
    return desiredSize(c) > 0;
  }

  function pullIfNeeded(c) {
    if (!shouldPull(c)) return;
    if (c.pulling) {
      c.pullAgain = true;
      return;
    }
    c.pulling = true;
    then(c.pull(c.object), () => {
      c.pulling = false;
      if (c.pullAgain) {
        c.pullAgain = false;
        pullIfNeeded(c);
      }
    }, (error) => errorController(c, error));
  }

  // A read: hands out a queued chunk or registers `request` (with `chunk`,
  // `close` and `error` steps) to be fulfilled later.
  function readInto(s, request) {
    s.disturbed = true;
    if (s.state === 'closed') {
      request.close();
    } else if (s.state === 'errored') {
      request.error(s.storedError);
    } else {
      const c = s.controller;
      if (c.queue.length > 0) {
        const chunk = dequeueValue(c);
        if (c.closeRequested && c.queue.length === 0) {
          clearAlgorithms(c);
          closeStream(s);
        } else {
          pullIfNeeded(c);
        }
        request.chunk(chunk);
      } else {
        s.reader.readRequests.push(request);
        pullIfNeeded(c);
      }
    }
  }

  function closeStream(s) {
    s.state = 'closed';
    const reader = s.reader;
    if (reader === null) return;
    reader.closed.resolve(undefined);
    const requests = reader.readRequests;
    reader.readRequests = [];
    for (const request of requests) request.close();
  }

  function errorStream(s, e) {
    s.state = 'errored';
    s.storedError = e;
    const reader = s.reader;
    if (reader === null) return;
    reader.closed.reject(e);
    handled(reader.closed.promise);
    const requests = reader.readRequests;
    reader.readRequests = [];
    for (const request of requests) request.error(e);
  }

  function cancelStream(s, reason) {
    s.disturbed = true;
    if (s.state === 'closed') return resolvedWith(undefined);
    if (s.state === 'errored') return rejectedWith(s.storedError);
    closeStream(s);
    const c = s.controller;
    resetQueue(c);
    const cancel = c.cancel;
    clearAlgorithms(c);
    return then(cancel(reason), () => undefined);
  }

  // ReadableStreamDefaultReader.
  let readerState;

  class ReadableStreamDefaultReader {
    #r;
    constructor(stream) {
      if (!isReadable(stream)) {
        throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'.");
      }
      const s = readableState(stream);
      if (locked(s)) {
        throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': ReadableStreamDefaultReader constructor can only accept readable streams that are not yet locked to a reader");
      }
      const r = { stream: s, readRequests: [], closed: deferred() };
      handled(r.closed.promise);
      s.reader = r;
      if (s.state === 'closed') r.closed.resolve(undefined);
      else if (s.state === 'errored') r.closed.reject(s.storedError);
      this.#r = r;
    }
    static {
      readerState = (reader) => reader.#r;
    }
    get closed() {
      return this.#r.closed.promise;
    }
    read() {
      const r = this.#r;
      if (r.stream === null) {
        return rejectedWith(new TypeError("Failed to execute 'read' on 'ReadableStreamDefaultReader': This readable stream reader has been released and cannot be used to read from its previous owner stream"));
      }
      const d = deferred();
      readInto(r.stream, {
        chunk: (value) => d.resolve({ value, done: false }),
        close: () => d.resolve({ value: undefined, done: true }),
        error: (e) => d.reject(e),
      });
      return d.promise;
    }
    releaseLock() {
      release(this.#r);
    }
    cancel(reason = undefined) {
      const r = this.#r;
      if (r.stream === null) {
        return rejectedWith(new TypeError("Failed to execute 'cancel' on 'ReadableStreamDefaultReader': This readable stream reader has been released and cannot be used to cancel its previous owner stream"));
      }
      return cancelStream(r.stream, reason);
    }
  }
  tag(ReadableStreamDefaultReader, 'ReadableStreamDefaultReader');

  function release(r) {
    const s = r.stream;
    if (s === null) return;
    const error = new TypeError('Releasing reader');
    if (s.state === 'readable') {
      r.closed.reject(error);
    } else {
      r.closed = deferred();
      r.closed.reject(error);
    }
    handled(r.closed.promise);
    s.reader = null;
    r.stream = null;
    const requests = r.readRequests;
    r.readRequests = [];
    for (const request of requests) request.error(error);
  }

  // A promise of the next chunk from reader state `r`.
  function readNext(r) {
    const d = deferred();
    readInto(r.stream, {
      chunk: (value) => d.resolve({ value, done: false }),
      close: () => d.resolve({ value: undefined, done: true }),
      error: (e) => d.reject(e),
    });
    return d.promise;
  }

  // Async iteration. https://streams.spec.whatwg.org/#rs-asynciterator
  const AsyncIteratorPrototype = Object.getPrototypeOf(
    Object.getPrototypeOf(async function* () {}).prototype);
  const ReadableStreamAsyncIteratorPrototype = Object.create(AsyncIteratorPrototype);
  let iteratorState;
  let makeIterator;
  {
    const states = new WeakMap();
    iteratorState = (it) => {
      const st = states.get(it);
      if (st === undefined) throw new TypeError('Illegal invocation');
      return st;
    };
    // Each call runs after the previous one settles, as the spec's
    // "ongoing promise" chaining requires.
    const chain = (st, step) => {
      const next = st.ongoing === null
        ? step()
        : then(st.ongoing, step, step);
      st.ongoing = handled(then(next, () => {
        st.ongoing = null;
      }, () => {
        st.ongoing = null;
      }));
      return next;
    };
    defineProperty(ReadableStreamAsyncIteratorPrototype, 'next', {
      value: function next() {
        let st;
        try {
          st = iteratorState(this);
        } catch (error) {
          return rejectedWith(error);
        }
        return chain(st, () => {
          if (st.done) return resolvedWith({ value: undefined, done: true });
          const r = st.reader;
          if (r.stream === null) {
            return rejectedWith(new TypeError('The reader has been released.'));
          }
          return then(readNext(r), (result) => {
            if (result.done) {
              st.done = true;
              release(r);
            }
            return result;
          }, (error) => {
            st.done = true;
            release(r);
            throw error;
          });
        });
      },
      writable: true,
      configurable: true,
    });
    defineProperty(ReadableStreamAsyncIteratorPrototype, 'return', {
      value: function (value) {
        let st;
        try {
          st = iteratorState(this);
        } catch (error) {
          return rejectedWith(error);
        }
        return chain(st, () => {
          if (st.done) return resolvedWith({ value, done: true });
          st.done = true;
          const r = st.reader;
          if (r.stream === null) return resolvedWith({ value, done: true });
          if (!st.preventCancel) {
            const result = cancelStream(r.stream, value);
            release(r);
            return then(result, () => ({ value, done: true }));
          }
          release(r);
          return resolvedWith({ value, done: true });
        });
      },
      writable: true,
      configurable: true,
    });
    defineProperty(ReadableStreamAsyncIteratorPrototype, Symbol.toStringTag, {
      value: 'ReadableStream AsyncIterator',
      configurable: true,
    });
    makeIterator = (reader, preventCancel) => {
      const it = Object.create(ReadableStreamAsyncIteratorPrototype);
      states.set(it, { reader, preventCancel, done: false, ongoing: null });
      return it;
    };
  }

  function iterate(stream, preventCancel) {
    const reader = new ReadableStreamDefaultReader(stream);
    return makeIterator(readerState(reader), preventCancel);
  }

  // A stream whose source forwards to the given algorithms, which receive
  // the public controller.
  function makeReadable(start, pull, cancel, hwm, size) {
    return new ReadableStream({
      start: (c) => start(c),
      pull: (c) => pull(c),
      cancel: (reason) => cancel(reason),
    }, { highWaterMark: hwm, size });
  }

  function readableFrom(asyncIterable) {
    if (isReadable(asyncIterable)) return asyncIterable;
    if (asyncIterable === null || asyncIterable === undefined) {
      throw new TypeError("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
    }
    let method = asyncIterable[Symbol.asyncIterator];
    let iterator;
    if (method === undefined || method === null) {
      method = asyncIterable[Symbol.iterator];
      if (typeof method !== 'function') {
        throw new TypeError("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
      }
      const syncIterator = ReflectApply(method, asyncIterable, []);
      iterator = {
        next: () => {
          const result = syncIterator.next();
          return then(resolvedWith(result.value), (value) => ({ value, done: !!result.done }));
        },
        return: (reason) => {
          const ret = syncIterator.return;
          return resolvedWith(typeof ret === 'function' ? ReflectApply(ret, syncIterator, [reason]) : undefined);
        },
      };
    } else {
      iterator = ReflectApply(method, asyncIterable, []);
    }
    if (iterator === null || typeof iterator !== 'object') {
      throw new TypeError("Failed to execute 'from' on 'ReadableStream': The iterator is not an object.");
    }
    const next = iterator.next;
    return makeReadable(noop, (c) => then(promiseCall(next, iterator, []), (result) => {
      if (result === null || typeof result !== 'object') {
        throw new TypeError('The iterator result is not an object.');
      }
      if (result.done) controllerState(c).object.close();
      else controllerState(c).object.enqueue(result.value);
    }), (reason) => {
      const ret = iterator.return;
      if (ret === undefined || ret === null) return resolvedWith(undefined);
      return then(promiseCall(ret, iterator, [reason]), (result) => {
        if (result === null || typeof result !== 'object') {
          throw new TypeError('The iterator result is not an object.');
        }
      });
    }, 0, () => 1);
  }

  // ReadableStreamDefaultTee.
  function tee(stream) {
    const s = readableState(stream);
    const reader = readerState(new ReadableStreamDefaultReader(stream));
    let reading = false;
    let readAgain = false;
    let canceled1 = false;
    let canceled2 = false;
    let reason1;
    let reason2;
    let c1;
    let c2;
    const cancelled = deferred();
    function pull() {
      if (reading) {
        readAgain = true;
        return resolvedWith(undefined);
      }
      reading = true;
      readInto(s, {
        chunk(value) {
          queueMicrotask(() => {
            readAgain = false;
            if (!canceled1) enqueueChunk(c1, value);
            if (!canceled2) enqueueChunk(c2, value);
            reading = false;
            if (readAgain) pull();
          });
        },
        close() {
          reading = false;
          if (!canceled1) closeController(c1);
          if (!canceled2) closeController(c2);
          if (!canceled1 || !canceled2) cancelled.resolve(undefined);
        },
        error() {
          reading = false;
        },
      });
      return resolvedWith(undefined);
    }
    function cancelBranch(first) {
      return (reason) => {
        if (first) {
          canceled1 = true;
          reason1 = reason;
        } else {
          canceled2 = true;
          reason2 = reason;
        }
        if (canceled1 && canceled2) {
          cancelled.resolve(cancelStream(s, [reason1, reason2]));
        }
        return cancelled.promise;
      };
    }
    const branch1 = makeReadable((c) => {
      c1 = controllerState(c);
    }, pull, cancelBranch(true), 1, () => 1);
    const branch2 = makeReadable((c) => {
      c2 = controllerState(c);
    }, pull, cancelBranch(false), 1, () => 1);
    then(reader.closed.promise, noop, (error) => {
      errorController(c1, error);
      errorController(c2, error);
      if (!canceled1 || !canceled2) cancelled.resolve(undefined);
    });
    return [branch1, branch2];
  }

  function pipeOptions(options, what) {
    const opts = dictionary(options, "execute '" + what + "' on 'ReadableStream'");
    const signal = opts.signal;
    if (signal !== undefined && !internals.isSignal(signal)) {
      throw new TypeError("Failed to execute '" + what + "' on 'ReadableStream': Failed to read the 'signal' property from 'StreamPipeOptions': Failed to convert value to 'AbortSignal'.");
    }
    return {
      preventClose: !!opts.preventClose,
      preventAbort: !!opts.preventAbort,
      preventCancel: !!opts.preventCancel,
      signal,
    };
  }

  // ReadableStreamPipeTo, written as an async loop: wait for the writer to
  // be ready, read a chunk, write it; propagate closing and errors both
  // ways as the options allow.
  function pipe(source, dest, { preventClose, preventAbort, preventCancel, signal }) {
    const s = readableState(source);
    const reader = readerState(new ReadableStreamDefaultReader(source));
    const writer = new WritableStreamDefaultWriter(dest);
    const w = writerState(writer);
    const ws = w.stream;
    s.disturbed = true;
    const done = deferred();
    let shuttingDown = false;
    let currentWrite = resolvedWith(undefined);

    function finalize(isError, error) {
      releaseWriter(w);
      release(reader);
      if (signal !== undefined) signal.removeEventListener('abort', onAbort);
      if (isError) done.reject(error);
      else done.resolve(undefined);
    }
    // Waits for pending writes, runs `action` if given, then finalizes.
    function shutdown(action, isError, error) {
      if (shuttingDown) return;
      shuttingDown = true;
      const waitWrites = () => (ws.state === 'writable' && !closeQueuedOrInFlight(ws)
        ? then(currentWrite, noop, noop)
        : resolvedWith(undefined));
      then(waitWrites(), () => {
        if (action === null) {
          finalize(isError, error);
          return;
        }
        then(promiseCall(action, undefined, []), () => finalize(isError, error),
          (newError) => finalize(true, newError));
      });
    }
    function onAbort() {
      const error = signal.reason;
      const actions = [];
      if (!preventAbort) {
        actions.push(() => (ws.state === 'writable' ? abortWritable(ws, error) : resolvedWith(undefined)));
      }
      if (!preventCancel) {
        actions.push(() => (s.state === 'readable' ? cancelStream(s, error) : resolvedWith(undefined)));
      }
      shutdown(() => PromiseRef.all(actions.map((a) => a())), true, error);
    }
    if (signal !== undefined) {
      if (signal.aborted) {
        onAbort();
        return done.promise;
      }
      signal.addEventListener('abort', onAbort);
    }

    function check() {
      // Errors and closing, forwards and backwards.
      if (s.state === 'errored') {
        if (!preventAbort) shutdown(() => abortWritable(ws, s.storedError), true, s.storedError);
        else shutdown(null, true, s.storedError);
        return true;
      }
      if (ws.state === 'errored' || ws.state === 'erroring') {
        const error = ws.storedError;
        if (!preventCancel) shutdown(() => cancelStream(s, error), true, error);
        else shutdown(null, true, error);
        return true;
      }
      if (s.state === 'closed') {
        if (!preventClose) shutdown(() => closeWithErrorPropagation(w), false);
        else shutdown(null, false);
        return true;
      }
      if (closeQueuedOrInFlight(ws) || ws.state === 'closed') {
        const error = new TypeError('the destination writable stream closed before all data could be piped to it');
        if (!preventCancel) shutdown(() => cancelStream(s, error), true, error);
        else shutdown(null, true, error);
        return true;
      }
      return false;
    }

    function step() {
      if (shuttingDown || check()) return;
      then(w.ready.promise, () => {
        if (shuttingDown || check()) return;
        readInto(s, {
          chunk(chunk) {
            currentWrite = handled(writeChunk(w, chunk));
            step();
          },
          close() {
            check();
          },
          error() {
            check();
          },
        });
      }, () => check());
    }
    then(reader.closed.promise, noop, () => {
      if (!shuttingDown) check();
    });
    then(w.closed.promise, noop, () => {
      if (!shuttingDown) check();
    });
    step();
    return done.promise;
  }

  // ---------------------------------------------------------------------
  // WritableStream.

  let writableState;
  let isWritable;

  class WritableStream {
    #s;
    constructor(underlyingSink = undefined, strategy = undefined) {
      const what = 'WritableStream';
      const sink = underlyingSink === null ? undefined : underlyingSink;
      if (sink !== undefined && typeof sink !== 'object' && typeof sink !== 'function') {
        throw new TypeError("Failed to construct 'WritableStream': The provided value is not of type 'UnderlyingSink'.");
      }
      if (sink !== undefined && sink.type !== undefined) {
        throw new RangeError("Failed to construct 'WritableStream': Invalid type is specified");
      }
      const strat = dictionary(strategy, "construct 'WritableStream'");
      const hwm = highWaterMark(strat, 1, what);
      const size = sizeAlgorithm(strat);
      const start = method(sink, 'start', what);
      const write = method(sink, 'write', what);
      const close = method(sink, 'close', what);
      const abort = method(sink, 'abort', what);
      this.#s = newWritableState();
      setUpWritableController(this.#s, {
        start: (c) => ReflectApply(start, sink, [c]),
        write: (chunk, c) => promiseCall(write, sink, [chunk, c]),
        close: () => promiseCall(close, sink, []),
        abort: (reason) => promiseCall(abort, sink, [reason]),
      }, hwm, size);
    }
    static {
      writableState = (stream) => stream.#s;
      isWritable = (value) => value !== null && typeof value === 'object' && #s in value;
    }
    get locked() {
      return checkWritable(this).writer !== null;
    }
    abort(reason = undefined) {
      if (!isWritable(this)) return rejectedWith(new TypeError('Illegal invocation'));
      const ws = writableState(this);
      if (ws.writer !== null) {
        return rejectedWith(new TypeError("Failed to execute 'abort' on 'WritableStream': Cannot abort a locked stream"));
      }
      return abortWritable(ws, reason);
    }
    close() {
      if (!isWritable(this)) return rejectedWith(new TypeError('Illegal invocation'));
      const ws = writableState(this);
      if (ws.writer !== null) {
        return rejectedWith(new TypeError("Failed to execute 'close' on 'WritableStream': Cannot close a locked stream"));
      }
      if (closeQueuedOrInFlight(ws)) {
        return rejectedWith(new TypeError("Failed to execute 'close' on 'WritableStream': Cannot close a stream that is already closed or closing"));
      }
      return closeWritable(ws);
    }
    getWriter() {
      checkWritable(this);
      return new WritableStreamDefaultWriter(this);
    }
  }
  tag(WritableStream, 'WritableStream');

  function checkWritable(stream) {
    if (!isWritable(stream)) throw new TypeError('Illegal invocation');
    return writableState(stream);
  }

  function newWritableState() {
    return {
      state: 'writable',
      storedError: undefined,
      writer: null,
      controller: null,
      writeRequests: [],
      inFlightWrite: null,
      closeRequest: null,
      inFlightClose: null,
      pendingAbort: null,
      backpressure: false,
    };
  }

  class WritableStreamDefaultController {
    #c;
    constructor(key, c) {
      illegal(key);
      this.#c = c;
    }
    get signal() {
      return this.#c.abortController.signal;
    }
    error(e = undefined) {
      const c = this.#c;
      if (c.stream.state !== 'writable') return;
      errorWritableController(c, e);
    }
  }
  tag(WritableStreamDefaultController, 'WritableStreamDefaultController');

  const CLOSE_SENTINEL = {};

  function setUpWritableController(ws, algorithms, hwm, size) {
    const c = {
      stream: ws,
      queue: [],
      queueTotalSize: 0,
      started: false,
      hwm,
      size,
      write: algorithms.write,
      close: algorithms.close,
      abort: algorithms.abort,
      abortController: new AbortControllerRef(),
      object: null,
    };
    c.object = new WritableStreamDefaultController(KEY, c);
    ws.controller = c;
    updateBackpressure(ws, writableDesiredSize(c) <= 0);
    let startResult;
    try {
      startResult = algorithms.start(c.object);
    } catch (error) {
      c.started = true;
      dealWithRejection(ws, error);
      return;
    }
    then(resolvedWith(startResult), () => {
      c.started = true;
      advanceQueueIfNeeded(c);
    }, (error) => {
      c.started = true;
      dealWithRejection(ws, error);
    });
  }

  function writableDesiredSize(c) {
    return c.hwm - c.queueTotalSize;
  }

  function clearWritableAlgorithms(c) {
    c.write = null;
    c.close = null;
    c.abort = null;
    c.size = null;
  }

  function closeQueuedOrInFlight(ws) {
    return ws.closeRequest !== null || ws.inFlightClose !== null;
  }

  function hasInFlight(ws) {
    return ws.inFlightWrite !== null || ws.inFlightClose !== null;
  }

  function updateBackpressure(ws, backpressure) {
    const w = ws.writer;
    if (w !== null && backpressure !== ws.backpressure) {
      if (backpressure) {
        w.ready = deferred();
        handled(w.ready.promise);
      } else {
        w.ready.resolve(undefined);
      }
    }
    ws.backpressure = backpressure;
  }

  function abortWritable(ws, reason) {
    if (ws.state === 'closed' || ws.state === 'errored') return resolvedWith(undefined);
    ws.controller.abortController.abort(reason);
    const state = ws.state;
    if (state === 'closed' || state === 'errored') return resolvedWith(undefined);
    if (ws.pendingAbort !== null) return ws.pendingAbort.deferred.promise;
    let wasAlreadyErroring = false;
    if (state === 'erroring') {
      wasAlreadyErroring = true;
      reason = undefined;
    }
    const d = deferred();
    ws.pendingAbort = { deferred: d, reason, wasAlreadyErroring };
    if (!wasAlreadyErroring) startErroring(ws, reason);
    return d.promise;
  }

  function closeWritable(ws) {
    const state = ws.state;
    if (state === 'closed' || state === 'errored') {
      return rejectedWith(new TypeError('The stream is not in the writable state and cannot be closed'));
    }
    const d = deferred();
    ws.closeRequest = d;
    const w = ws.writer;
    if (w !== null && ws.backpressure && state === 'writable') w.ready.resolve(undefined);
    const c = ws.controller;
    enqueueValue(c, CLOSE_SENTINEL, 0);
    advanceQueueIfNeeded(c);
    return d.promise;
  }

  function dealWithRejection(ws, error) {
    if (ws.state === 'writable') {
      startErroring(ws, error);
      return;
    }
    finishErroring(ws);
  }

  function startErroring(ws, reason) {
    const c = ws.controller;
    ws.state = 'erroring';
    ws.storedError = reason;
    const w = ws.writer;
    if (w !== null) writerEnsureReadyRejected(w, reason);
    if (!hasInFlight(ws) && c.started) finishErroring(ws);
  }

  function finishErroring(ws) {
    ws.state = 'errored';
    const c = ws.controller;
    resetQueue(c);
    const error = ws.storedError;
    for (const request of ws.writeRequests) request.reject(error);
    ws.writeRequests = [];
    const abort = ws.pendingAbort;
    if (abort === null) {
      rejectCloseAndClosedPromiseIfNeeded(ws);
      return;
    }
    ws.pendingAbort = null;
    if (abort.wasAlreadyErroring) {
      abort.deferred.reject(error);
      rejectCloseAndClosedPromiseIfNeeded(ws);
      return;
    }
    const result = c.abort === null ? resolvedWith(undefined) : c.abort(abort.reason);
    clearWritableAlgorithms(c);
    then(result, () => {
      abort.deferred.resolve(undefined);
      rejectCloseAndClosedPromiseIfNeeded(ws);
    }, (reason) => {
      abort.deferred.reject(reason);
      rejectCloseAndClosedPromiseIfNeeded(ws);
    });
  }

  function rejectCloseAndClosedPromiseIfNeeded(ws) {
    if (ws.closeRequest !== null) {
      ws.closeRequest.reject(ws.storedError);
      ws.closeRequest = null;
    }
    const w = ws.writer;
    if (w !== null) {
      w.closed.reject(ws.storedError);
      handled(w.closed.promise);
    }
  }

  function errorWritableController(c, error) {
    const ws = c.stream;
    clearWritableAlgorithms(c);
    startErroring(ws, error);
  }

  function writeChunk(w, chunk) {
    const ws = w.stream;
    const c = ws.controller;
    let chunkSize;
    try {
      chunkSize = c.size === null ? 1 : c.size(chunk);
    } catch (error) {
      if (ws.state === 'writable') errorWritableController(c, error);
      chunkSize = 1;
    }
    if (ws !== w.stream) return rejectedWith(new TypeError('The writer has been released.'));
    const state = ws.state;
    if (state === 'errored') return rejectedWith(ws.storedError);
    if (closeQueuedOrInFlight(ws) || state === 'closed') {
      return rejectedWith(new TypeError('The stream is closing or closed and cannot be written to'));
    }
    if (state === 'erroring') return rejectedWith(ws.storedError);
    const d = deferred();
    ws.writeRequests.push(d);
    try {
      enqueueValue(c, chunk, chunkSize);
    } catch (error) {
      errorWritableController(c, error);
      return d.promise;
    }
    if (!closeQueuedOrInFlight(ws) && ws.state === 'writable') {
      updateBackpressure(ws, writableDesiredSize(c) <= 0);
    }
    advanceQueueIfNeeded(c);
    return d.promise;
  }

  function advanceQueueIfNeeded(c) {
    const ws = c.stream;
    if (!c.started || ws.inFlightWrite !== null) return;
    if (ws.state === 'erroring') {
      finishErroring(ws);
      return;
    }
    if (c.queue.length === 0) return;
    const value = c.queue[0].value;
    if (value === CLOSE_SENTINEL) {
      ws.inFlightClose = ws.closeRequest;
      ws.closeRequest = null;
      dequeueValue(c);
      const close = c.close;
      clearWritableAlgorithms(c);
      then(close(), () => {
        ws.inFlightClose.resolve(undefined);
        ws.inFlightClose = null;
        if (ws.state === 'erroring') {
          ws.storedError = undefined;
          if (ws.pendingAbort !== null) {
            ws.pendingAbort.deferred.resolve(undefined);
            ws.pendingAbort = null;
          }
        }
        ws.state = 'closed';
        const w = ws.writer;
        if (w !== null) w.closed.resolve(undefined);
      }, (error) => {
        ws.inFlightClose.reject(error);
        ws.inFlightClose = null;
        if (ws.pendingAbort !== null) {
          ws.pendingAbort.deferred.reject(error);
          ws.pendingAbort = null;
        }
        dealWithRejection(ws, error);
      });
      return;
    }
    ws.inFlightWrite = ws.writeRequests.shift();
    then(c.write(value, c.object), () => {
      ws.inFlightWrite.resolve(undefined);
      ws.inFlightWrite = null;
      const state = ws.state;
      dequeueValue(c);
      if (!closeQueuedOrInFlight(ws) && state === 'writable') {
        updateBackpressure(ws, writableDesiredSize(c) <= 0);
      }
      advanceQueueIfNeeded(c);
    }, (error) => {
      if (ws.state === 'writable') clearWritableAlgorithms(c);
      ws.inFlightWrite.reject(error);
      ws.inFlightWrite = null;
      dealWithRejection(ws, error);
    });
  }

  // WritableStreamDefaultWriter.
  let writerState;

  class WritableStreamDefaultWriter {
    #w;
    constructor(stream) {
      if (!isWritable(stream)) {
        throw new TypeError("Failed to construct 'WritableStreamDefaultWriter': parameter 1 is not of type 'WritableStream'.");
      }
      const ws = writableState(stream);
      if (ws.writer !== null) {
        throw new TypeError("Failed to construct 'WritableStreamDefaultWriter': This stream has already been locked for exclusive writing by another writer");
      }
      const w = { stream: ws, ready: deferred(), closed: deferred() };
      handled(w.ready.promise);
      handled(w.closed.promise);
      ws.writer = w;
      const state = ws.state;
      if (state === 'writable') {
        if (!closeQueuedOrInFlight(ws) && ws.backpressure) {
          // ready stays pending.
        } else {
          w.ready.resolve(undefined);
        }
      } else if (state === 'erroring') {
        w.ready.reject(ws.storedError);
      } else if (state === 'closed') {
        w.ready.resolve(undefined);
        w.closed.resolve(undefined);
      } else {
        w.ready.reject(ws.storedError);
        w.closed.reject(ws.storedError);
      }
      this.#w = w;
    }
    static {
      writerState = (writer) => writer.#w;
    }
    get closed() {
      return this.#w.closed.promise;
    }
    get ready() {
      return this.#w.ready.promise;
    }
    get desiredSize() {
      const ws = this.#w.stream;
      if (ws === null) {
        throw new TypeError("Failed to read the 'desiredSize' property from 'WritableStreamDefaultWriter': This writable stream writer has been released and cannot be used to get the desired size");
      }
      if (ws.state === 'errored' || ws.state === 'erroring') return null;
      if (ws.state === 'closed') return 0;
      return writableDesiredSize(ws.controller);
    }
    abort(reason = undefined) {
      const ws = this.#w.stream;
      if (ws === null) return rejectedWith(new TypeError('The writer has been released.'));
      return abortWritable(ws, reason);
    }
    close() {
      const ws = this.#w.stream;
      if (ws === null) return rejectedWith(new TypeError('The writer has been released.'));
      if (closeQueuedOrInFlight(ws)) {
        return rejectedWith(new TypeError("Failed to execute 'close' on 'WritableStreamDefaultWriter': Cannot close a stream that is already closed or closing"));
      }
      return closeWritable(ws);
    }
    releaseLock() {
      releaseWriter(this.#w);
    }
    write(chunk = undefined) {
      if (this.#w.stream === null) {
        return rejectedWith(new TypeError('The writer has been released.'));
      }
      return writeChunk(this.#w, chunk);
    }
  }
  tag(WritableStreamDefaultWriter, 'WritableStreamDefaultWriter');

  function writerEnsureReadyRejected(w, error) {
    if (!w.ready.settled) w.ready.reject(error);
    else {
      w.ready = deferred();
      w.ready.reject(error);
    }
    handled(w.ready.promise);
  }

  function releaseWriter(w) {
    const ws = w.stream;
    if (ws === null) return;
    const error = new TypeError('Writer was released and can no longer be used to monitor the stream\'s closedness');
    writerEnsureReadyRejected(w, error);
    if (!w.closed.settled) w.closed.reject(error);
    else {
      w.closed = deferred();
      w.closed.reject(error);
    }
    handled(w.closed.promise);
    ws.writer = null;
    w.stream = null;
  }

  function closeWithErrorPropagation(w) {
    const ws = w.stream;
    if (closeQueuedOrInFlight(ws) || ws.state === 'closed') return resolvedWith(undefined);
    if (ws.state === 'errored') return rejectedWith(ws.storedError);
    return closeWritable(ws);
  }

  // ---------------------------------------------------------------------
  // TransformStream.

  class TransformStream {
    #t;
    constructor(transformer = undefined, writableStrategy = undefined, readableStrategy = undefined) {
      const what = 'TransformStream';
      const tr = transformer === null ? undefined : transformer;
      if (tr !== undefined && (tr.readableType !== undefined || tr.writableType !== undefined)) {
        throw new RangeError("Failed to construct 'TransformStream': Invalid type is specified");
      }
      const ws = dictionary(writableStrategy, "construct 'TransformStream'");
      const rs = dictionary(readableStrategy, "construct 'TransformStream'");
      const writableHwm = highWaterMark(ws, 1, what);
      const writableSize = sizeAlgorithm(ws);
      const readableHwm = highWaterMark(rs, 0, what);
      const readableSize = sizeAlgorithm(rs);
      const start = method(tr, 'start', what);
      const transform = tr === undefined || tr.transform === undefined ? null : method(tr, 'transform', what);
      const flush = method(tr, 'flush', what);
      const cancel = method(tr, 'cancel', what);
      const t = {
        readable: null,
        writable: null,
        backpressure: false,
        backpressureChange: null,
        readableController: null,
        controller: null,
        transform: null,
        flush: null,
        cancel: null,
        finish: null,
      };
      this.#t = t;
      const startPromise = deferred();
      setUpTransform(t, startPromise.promise, writableHwm, writableSize, readableHwm, readableSize);
      t.controller = new TransformStreamDefaultController(KEY, t);
      t.transform = transform === null
        ? (chunk) => {
          try {
            transformEnqueue(t, chunk);
            return resolvedWith(undefined);
          } catch (error) {
            return rejectedWith(error);
          }
        }
        : (chunk) => promiseCall(transform, tr, [chunk, t.controller]);
      t.flush = () => promiseCall(flush, tr, [t.controller]);
      t.cancel = (reason) => promiseCall(cancel, tr, [reason]);
      try {
        startPromise.resolve(ReflectApply(start, tr, [t.controller]));
      } catch (error) {
        startPromise.reject(error);
      }
    }
    get readable() {
      return this.#t.readable;
    }
    get writable() {
      return this.#t.writable;
    }
  }
  tag(TransformStream, 'TransformStream');

  class TransformStreamDefaultController {
    #t;
    constructor(key, t) {
      illegal(key);
      this.#t = t;
    }
    get desiredSize() {
      return desiredSize(this.#t.readableController);
    }
    enqueue(chunk = undefined) {
      transformEnqueue(this.#t, chunk);
    }
    error(reason = undefined) {
      transformError(this.#t, reason);
    }
    terminate() {
      const t = this.#t;
      closeController(t.readableController);
      transformErrorWritableAndUnblockWrite(t, new TypeError('The transform stream has been terminated'));
    }
  }
  tag(TransformStreamDefaultController, 'TransformStreamDefaultController');

  function setUpTransform(t, startPromise, writableHwm, writableSize, readableHwm, readableSize) {
    t.writable = new WritableStream({
      start: () => startPromise,
      write: (chunk) => {
        if (t.backpressure) {
          return then(t.backpressureChange.promise, () => {
            const ws = writableState(t.writable);
            if (ws.state === 'erroring') throw ws.storedError;
            return performTransform(t, chunk);
          });
        }
        return performTransform(t, chunk);
      },
      close: () => {
        if (t.finish !== null) return t.finish.promise;
        t.finish = deferred();
        const rs = readableState(t.readable);
        then(t.flush(), () => {
          if (rs.state === 'errored') {
            t.finish.reject(rs.storedError);
          } else {
            closeController(t.readableController);
            t.finish.resolve(undefined);
          }
        }, (error) => {
          errorController(t.readableController, error);
          t.finish.reject(error);
        });
        return t.finish.promise;
      },
      abort: (reason) => {
        if (t.finish !== null) return t.finish.promise;
        t.finish = deferred();
        const rs = readableState(t.readable);
        then(t.cancel(reason), () => {
          if (rs.state === 'errored') {
            t.finish.reject(rs.storedError);
          } else {
            errorController(t.readableController, reason);
            t.finish.resolve(undefined);
          }
        }, (error) => {
          errorController(t.readableController, error);
          t.finish.reject(error);
        });
        return t.finish.promise;
      },
    }, { highWaterMark: writableHwm, size: writableSize });
    t.readable = new ReadableStream({
      start: (c) => {
        t.readableController = controllerState(c);
        return startPromise;
      },
      pull: () => {
        setBackpressure(t, false);
        return t.backpressureChange.promise;
      },
      cancel: (reason) => {
        if (t.finish !== null) return t.finish.promise;
        t.finish = deferred();
        const ws = writableState(t.writable);
        then(t.cancel(reason), () => {
          if (ws.state === 'errored') {
            t.finish.reject(ws.storedError);
          } else {
            errorWritableIfWritable(ws, reason);
            unblockWrite(t);
            t.finish.resolve(undefined);
          }
        }, (error) => {
          errorWritableIfWritable(ws, error);
          unblockWrite(t);
          t.finish.reject(error);
        });
        return t.finish.promise;
      },
    }, { highWaterMark: readableHwm, size: readableSize });
    setBackpressure(t, true);
  }

  function errorWritableIfWritable(ws, error) {
    if (ws.state === 'writable') errorWritableController(ws.controller, error);
  }

  function setBackpressure(t, backpressure) {
    if (t.backpressureChange !== null) t.backpressureChange.resolve(undefined);
    t.backpressureChange = deferred();
    t.backpressure = backpressure;
  }

  function unblockWrite(t) {
    if (t.backpressure) setBackpressure(t, false);
  }

  function performTransform(t, chunk) {
    return then(t.transform(chunk), undefined, (error) => {
      transformError(t, error);
      throw error;
    });
  }

  function transformEnqueue(t, chunk) {
    const c = t.readableController;
    if (!canCloseOrEnqueue(c)) {
      throw new TypeError("Failed to execute 'enqueue' on 'TransformStreamDefaultController': Readable side is not in a state that permits enqueue");
    }
    try {
      enqueueChunk(c, chunk);
    } catch (error) {
      transformErrorWritableAndUnblockWrite(t, error);
      throw readableState(t.readable).storedError;
    }
    const backpressure = !shouldPull(c) && desiredSize(c) <= 0;
    if (backpressure !== t.backpressure) setBackpressure(t, true);
  }

  function transformError(t, error) {
    errorController(t.readableController, error);
    transformErrorWritableAndUnblockWrite(t, error);
  }

  function transformErrorWritableAndUnblockWrite(t, error) {
    t.transform = () => resolvedWith(undefined);
    t.flush = () => resolvedWith(undefined);
    errorWritableIfWritable(writableState(t.writable), error);
    unblockWrite(t);
  }

  // ---------------------------------------------------------------------
  // Queuing strategies.

  const byteLengthSize = function size(chunk) {
    return chunk.byteLength;
  };
  const countSize = function size() {
    return 1;
  };

  class ByteLengthQueuingStrategy {
    #hwm;
    constructor(init) {
      if (init === undefined || init === null || init.highWaterMark === undefined) {
        throw new TypeError("Failed to construct 'ByteLengthQueuingStrategy': Failed to read the 'highWaterMark' property from 'QueuingStrategyInit': Required member is undefined.");
      }
      this.#hwm = Number(init.highWaterMark);
    }
    get highWaterMark() {
      return this.#hwm;
    }
    get size() {
      return byteLengthSize;
    }
  }
  tag(ByteLengthQueuingStrategy, 'ByteLengthQueuingStrategy');

  class CountQueuingStrategy {
    #hwm;
    constructor(init) {
      if (init === undefined || init === null || init.highWaterMark === undefined) {
        throw new TypeError("Failed to construct 'CountQueuingStrategy': Failed to read the 'highWaterMark' property from 'QueuingStrategyInit': Required member is undefined.");
      }
      this.#hwm = Number(init.highWaterMark);
    }
    get highWaterMark() {
      return this.#hwm;
    }
    get size() {
      return countSize;
    }
  }
  tag(CountQueuingStrategy, 'CountQueuingStrategy');

  // ---------------------------------------------------------------------
  // TextEncoderStream and TextDecoderStream.

  class TextEncoderStream {
    #transform;
    constructor() {
      const encoder = new TextEncoderRef();
      let pendingHigh = '';
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          let text = pendingHigh + String(chunk);
          pendingHigh = '';
          const last = text.charCodeAt(text.length - 1);
          if (last >= 0xD800 && last <= 0xDBFF) {
            pendingHigh = text.slice(-1);
            text = text.slice(0, -1);
          }
          if (text !== '') controller.enqueue(encoder.encode(text));
        },
        flush(controller) {
          if (pendingHigh !== '') controller.enqueue(new Uint8Array([0xEF, 0xBF, 0xBD]));
        },
      });
    }
    get encoding() {
      return 'utf-8';
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }
  tag(TextEncoderStream, 'TextEncoderStream');

  class TextDecoderStream {
    #decoder;
    #transform;
    constructor(label = 'utf-8', options = undefined) {
      const decoder = new TextDecoderRef(label, options);
      this.#decoder = decoder;
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          const text = decoder.decode(chunk, { stream: true });
          if (text !== '') controller.enqueue(text);
        },
        flush(controller) {
          const text = decoder.decode();
          if (text !== '') controller.enqueue(text);
        },
      });
    }
    get encoding() {
      return this.#decoder.encoding;
    }
    get fatal() {
      return this.#decoder.fatal;
    }
    get ignoreBOM() {
      return this.#decoder.ignoreBOM;
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }
  tag(TextDecoderStream, 'TextDecoderStream');

  internals.expose({
    ReadableStream,
    ReadableStreamDefaultReader,
    ReadableStreamDefaultController,
    ReadableByteStreamController,
    WritableStream,
    WritableStreamDefaultWriter,
    WritableStreamDefaultController,
    TransformStream,
    TransformStreamDefaultController,
    ByteLengthQueuingStrategy,
    CountQueuingStrategy,
    TextEncoderStream,
    TextDecoderStream,
  });

  // For fetch.js: a stream whose chunks come from `pull` (which enqueues on
  // the controller state it is given, and returns a promise), a check, and
  // a way to read a whole stream.
  Object.assign(internals, {
    isReadableStream: isReadable,
    readableStreamLocked: (stream) => locked(readableState(stream)),
    readableStreamDisturbed: (stream) => readableState(stream).disturbed,
    // A stream that produces the bytes `take()` promises as one chunk, and
    // calls `onCancel` if cancelled first.
    bytesStream(take, onCancel) {
      let taken = false;
      return makeReadable(noop, (c) => {
        if (taken) return resolvedWith(undefined);
        taken = true;
        const controller = controllerState(c);
        return then(take(), (bytes) => {
          if (bytes.length > 0) enqueueChunk(controller, new Uint8Array(bytes));
          closeController(controller);
        }, (error) => errorController(controller, error));
      }, () => {
        if (!taken) {
          taken = true;
          onCancel();
        }
        return resolvedWith(undefined);
      }, 0, () => 1);
    },
    // Reads all of `stream` (which must not be locked) as one Uint8Array.
    readAllBytes(stream) {
      const reader = readerState(new ReadableStreamDefaultReader(stream));
      const chunks = [];
      let total = 0;
      const d = deferred();
      const step = () => {
        readInto(reader.stream, {
          chunk(chunk) {
            if (!(chunk instanceof Uint8Array)) {
              const error = new TypeError('Received non-Uint8Array chunk');
              cancelStream(reader.stream, error);
              d.reject(error);
              return;
            }
            chunks.push(chunk);
            total += chunk.length;
            queueMicrotask(step);
          },
          close() {
            const out = new Uint8Array(total);
            let at = 0;
            for (const chunk of chunks) {
              out.set(chunk, at);
              at += chunk.length;
            }
            d.resolve(out);
          },
          error(e) {
            d.reject(e);
          },
        });
      };
      step();
      return d.promise;
    },
  });
})
