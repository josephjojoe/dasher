// fetch() and its classes: Headers, Blob, File, FormData, Request and
// Response.
// https://fetch.spec.whatwg.org/
// https://w3c.github.io/FileAPI/
// https://xhr.spec.whatwg.org/#interface-formdata
//
// Requests go to Rust through `native.netSend`, which sends them through
// the page's network session and calls back into `onResponse`, `onBody`
// and `onError` (see net.rs). Bodies arrive whole; `response.body` is a
// ReadableStream (streams.js, loaded on first use) that gives them as one
// chunk.
//
// XMLHttpRequest lives in xhr.js and builds on the helpers this file adds
// to `internals`.
(function (native, internals) {
  'use strict';

  const U8 = Uint8Array;
  const ArrayBufferRef = ArrayBuffer;
  const isView = ArrayBuffer.isView;
  const JSONParse = JSON.parse;
  const JSONStringify = JSON.stringify;
  const PromiseRef = Promise;
  const PromiseThen = Promise.prototype.then;
  const PromiseCatch = Promise.prototype.catch;
  const ReflectApply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const AbortSignalRef = globalThis.AbortSignal;
  const AbortControllerRef = globalThis.AbortController;
  const usv = internals.usv;
  const isSignal = internals.isSignal;
  const signalAddAlgorithm = internals.signalAddAlgorithm;
  const netSend = native.netSend;
  const netAbort = native.netAbort;
  const decodeUtf8 = native.decodeUtf8;
  const encodeUtf8 = native.encodeUtf8;
  const resolveUrl = native.resolveUrl;
  const EMPTY = new U8(0);
  const noop = () => {};

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

  function then(promise, onFulfilled, onRejected) {
    return ReflectApply(PromiseThen, promise, [onFulfilled, onRejected]);
  }

  // A promise whose rejection is never reported as unhandled: callers that
  // care chain their own handlers off it.
  function deferred() {
    let resolve;
    let reject;
    const promise = new PromiseRef((res, rej) => {
      resolve = res;
      reject = rej;
    });
    ReflectApply(PromiseCatch, promise, [noop]);
    return { promise, resolve, reject };
  }

  // The page's URL, and the base URL relative URLs resolve against. Both
  // are read at call time, so history.pushState() is followed.
  function documentURL() {
    return internals.documentURL ? internals.documentURL() : 'about:blank';
  }
  function baseURL() {
    return internals.baseURL ? internals.baseURL() : documentURL();
  }
  // Resolves `input` against the base URL; null if it is not a valid URL.
  function parseURL(input) {
    const url = resolveUrl(usv(input), baseURL());
    return url === undefined ? null : url;
  }

  function stripFragment(url) {
    const i = url.indexOf('#');
    return i < 0 ? url : url.slice(0, i);
  }

  const encode = (text) => new U8(encodeUtf8(usv(text)));
  const decode = (bytes) => decodeUtf8(bytes);

  function isBufferSource(value) {
    return value instanceof ArrayBufferRef || isView(value);
  }

  // A copy of the bytes of an ArrayBuffer or view.
  function copyBytes(value) {
    if (value instanceof ArrayBufferRef) return new U8(value.slice(0));
    return new U8(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
  }

  function concat(chunks) {
    let length = 0;
    for (const chunk of chunks) length += chunk.length;
    const out = new U8(length);
    let offset = 0;
    for (const chunk of chunks) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  }

  // HTTP syntax.

  const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const isToken = (s) => TOKEN.test(s);
  const normalizeValue = (value) => String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
  const isValidValue = (value) => !/[\0\r\n]|[^\0-\xFF]/.test(value);

  const FORBIDDEN_REQUEST_HEADERS = new Set([
    'accept-charset', 'accept-encoding', 'access-control-request-headers',
    'access-control-request-method', 'connection', 'content-length', 'cookie', 'cookie2',
    'date', 'dnt', 'expect', 'host', 'keep-alive', 'origin', 'referer', 'set-cookie', 'te',
    'trailer', 'transfer-encoding', 'upgrade', 'via',
  ]);

  // Headers scripts may not set: the browser (here, the session) owns them.
  function isForbiddenRequestHeader(lower) {
    return FORBIDDEN_REQUEST_HEADERS.has(lower) || lower.startsWith('proxy-') ||
      lower.startsWith('sec-');
  }

  function isForbiddenResponseHeader(lower) {
    return lower === 'set-cookie' || lower === 'set-cookie2';
  }

  const FORBIDDEN_METHODS = new Set(['CONNECT', 'TRACE', 'TRACK']);
  const NORMALIZED_METHODS = new Set(['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT']);

  // Validates and normalizes a request method. Returns null for one that
  // is not a token, and throws for a forbidden one.
  function normalizeMethod(method) {
    method = String(method);
    if (!isToken(method)) return null;
    const upper = method.toUpperCase();
    if (FORBIDDEN_METHODS.has(upper)) return undefined;
    return NORMALIZED_METHODS.has(upper) ? upper : method;
  }

  // Headers. https://fetch.spec.whatwg.org/#headers-class

  let headerList;
  let setGuard;

  class Headers {
    // [lowercase name, value] pairs in insertion order.
    #list = [];
    // 'none', 'request', 'response' or 'immutable'.
    #guard = 'none';
    constructor(init) {
      if (init !== undefined) fillHeaders(this, init, "construct 'Headers'");
    }
    static {
      headerList = (headers) => headers.#list;
      setGuard = (headers, guard) => {
        headers.#guard = guard;
      };
    }
    #check(name, value, what) {
      name = String(name);
      if (!isToken(name)) {
        throw new TypeError('Failed to execute ' + what + ": Invalid name");
      }
      if (value !== undefined && !isValidValue(value)) {
        throw new TypeError('Failed to execute ' + what + ': Invalid value');
      }
      if (this.#guard === 'immutable') {
        throw new TypeError('Failed to execute ' + what + ': Headers are immutable');
      }
      const lower = name.toLowerCase();
      if (this.#guard === 'request' && isForbiddenRequestHeader(lower)) return null;
      if (this.#guard === 'response' && isForbiddenResponseHeader(lower)) return null;
      return lower;
    }
    append(name, value) {
      requireArgs(arguments, 2, "execute 'append' on 'Headers'");
      value = normalizeValue(value);
      const lower = this.#check(name, value, "'append' on 'Headers'");
      if (lower !== null) this.#list.push([lower, value]);
    }
    delete(name) {
      requireArgs(arguments, 1, "execute 'delete' on 'Headers'");
      const lower = this.#check(name, undefined, "'delete' on 'Headers'");
      if (lower !== null) this.#list = this.#list.filter((pair) => pair[0] !== lower);
    }
    get(name) {
      requireArgs(arguments, 1, "execute 'get' on 'Headers'");
      name = String(name);
      if (!isToken(name)) throw new TypeError("Failed to execute 'get' on 'Headers': Invalid name");
      return getHeader(this.#list, name.toLowerCase());
    }
    getSetCookie() {
      return this.#list.filter((pair) => pair[0] === 'set-cookie').map((pair) => pair[1]);
    }
    has(name) {
      requireArgs(arguments, 1, "execute 'has' on 'Headers'");
      name = String(name);
      if (!isToken(name)) throw new TypeError("Failed to execute 'has' on 'Headers': Invalid name");
      const lower = name.toLowerCase();
      return this.#list.some((pair) => pair[0] === lower);
    }
    set(name, value) {
      requireArgs(arguments, 2, "execute 'set' on 'Headers'");
      value = normalizeValue(value);
      const lower = this.#check(name, value, "'set' on 'Headers'");
      if (lower === null) return;
      const i = this.#list.findIndex((pair) => pair[0] === lower);
      if (i < 0) {
        this.#list.push([lower, value]);
      } else {
        this.#list[i] = [lower, value];
        this.#list = this.#list.filter((pair, j) => j <= i || pair[0] !== lower);
      }
    }
    forEach(callback, thisArg) {
      requireArgs(arguments, 1, "execute 'forEach' on 'Headers'");
      for (const [name, value] of sortedHeaders(this.#list)) {
        ReflectApply(callback, thisArg, [value, name, this]);
      }
    }
    *entries() {
      yield* sortedHeaders(this.#list);
    }
    *keys() {
      for (const pair of sortedHeaders(this.#list)) yield pair[0];
    }
    *values() {
      for (const pair of sortedHeaders(this.#list)) yield pair[1];
    }
    [Symbol.iterator]() {
      return this.entries();
    }
  }
  tag(Headers, 'Headers');

  // The combined value of every header called `lower`, or null.
  function getHeader(list, lower) {
    let value = null;
    for (const pair of list) {
      if (pair[0] === lower) value = value === null ? pair[1] : value + ', ' + pair[1];
    }
    return value;
  }

  // The headers as scripts iterate them: sorted, combined, but with each
  // Set-Cookie on its own.
  function sortedHeaders(list) {
    const names = [...new Set(list.map((pair) => pair[0]))].sort();
    const out = [];
    for (const name of names) {
      if (name === 'set-cookie') {
        for (const pair of list) if (pair[0] === name) out.push([name, pair[1]]);
      } else {
        out.push([name, getHeader(list, name)]);
      }
    }
    return out;
  }

  function fillHeaders(headers, init, what) {
    if (typeof init !== 'object' && typeof init !== 'function') {
      throw new TypeError('Failed to ' + what + ": The provided value is not of type '(record<ByteString, ByteString> or sequence<sequence<ByteString>>)'.");
    }
    if (init === null) return;
    if (typeof init[Symbol.iterator] === 'function') {
      for (const pair of init) {
        const items = typeof pair === 'string' ? null : [...pair];
        if (items === null || items.length !== 2) {
          throw new TypeError('Failed to ' + what + ': Invalid value');
        }
        headers.append(items[0], items[1]);
      }
    } else {
      for (const key of Object.keys(init)) headers.append(key, init[key]);
    }
  }

  // Headers from a flat [name, value, ...] list, as Rust delivers them.
  function headersFromFlat(flat, guard) {
    const headers = new Headers();
    const list = headerList(headers);
    for (let i = 0; i + 1 < flat.length; i += 2) list.push([flat[i], flat[i + 1]]);
    setGuard(headers, guard);
    return headers;
  }

  function flatHeaders(list) {
    const flat = [];
    for (const pair of list) flat.push(pair[0], pair[1]);
    return flat;
  }

  // Blob and File. https://w3c.github.io/FileAPI/#blob-section

  const BLOB_KEY = Symbol('Blob');
  let blobBytes;

  function blobType(type) {
    type = String(type);
    return /^[\x20-\x7E]*$/.test(type) ? type.toLowerCase() : '';
  }

  class Blob {
    #bytes;
    #type;
    constructor(blobParts, options) {
      if (blobParts === BLOB_KEY) {
        this.#bytes = options.bytes;
        this.#type = options.type;
        return;
      }
      const opts = dictionary(options, "construct 'Blob'");
      const chunks = [];
      if (blobParts !== undefined) {
        if (blobParts === null || typeof blobParts !== 'object' ||
            typeof blobParts[Symbol.iterator] !== 'function') {
          throw new TypeError("Failed to construct 'Blob': The provided value cannot be converted to a sequence.");
        }
        const native = opts.endings === 'native';
        for (const part of blobParts) {
          if (part instanceof Blob) {
            chunks.push(part.#bytes);
          } else if (isBufferSource(part)) {
            chunks.push(copyBytes(part));
          } else {
            const text = String(part);
            chunks.push(encode(native ? text.replace(/\r\n?/g, '\n') : text));
          }
        }
      }
      this.#bytes = chunks.length === 1 ? chunks[0] : concat(chunks);
      this.#type = opts.type === undefined ? '' : blobType(opts.type);
    }
    static {
      blobBytes = (blob) => blob.#bytes;
    }
    get size() {
      return this.#bytes.length;
    }
    get type() {
      return this.#type;
    }
    slice(start, end, contentType) {
      const size = this.#bytes.length;
      const clamp = (n, fallback) => {
        if (n === undefined) return fallback;
        n = Math.trunc(Number(n)) || 0;
        return n < 0 ? Math.max(size + n, 0) : Math.min(n, size);
      };
      const from = clamp(start, 0);
      const to = clamp(end, size);
      const bytes = this.#bytes.subarray(from, Math.max(from, to));
      return makeBlob(bytes, contentType === undefined ? '' : blobType(contentType));
    }
    text() {
      return PromiseRef.resolve(decode(this.#bytes));
    }
    arrayBuffer() {
      return PromiseRef.resolve(copyBytes(this.#bytes).buffer);
    }
    bytes() {
      return PromiseRef.resolve(copyBytes(this.#bytes));
    }
    stream() {
      return makeStream(() => PromiseRef.resolve(this.#bytes), noop);
    }
  }
  tag(Blob, 'Blob');

  // A Blob over `bytes`, which must not change afterwards.
  function makeBlob(bytes, type) {
    return new Blob(BLOB_KEY, { bytes, type });
  }

  class File extends Blob {
    #name;
    #lastModified;
    constructor(fileBits, fileName, options) {
      requireArgs(arguments, 2, "construct 'File'");
      const opts = dictionary(options, "construct 'File'");
      super(fileBits, opts);
      this.#name = usv(fileName);
      this.#lastModified = opts.lastModified === undefined ? Date.now() : Math.trunc(Number(opts.lastModified)) || 0;
    }
    get name() {
      return this.#name;
    }
    get lastModified() {
      return this.#lastModified;
    }
    get webkitRelativePath() {
      return '';
    }
  }
  tag(File, 'File');

  // FileList. https://w3c.github.io/FileAPI/#filelist-section
  // Only the engine makes one; nothing here selects files, so the list of
  // an <input type=file> is always empty.

  const FILE_LIST_KEY = Symbol('FileList');

  class FileList {
    #files;
    constructor(key, files) {
      if (key !== FILE_LIST_KEY) throw new TypeError('Illegal constructor');
      this.#files = files;
      files.forEach((file, i) => {
        Object.defineProperty(this, i, { value: file, enumerable: true, configurable: true });
      });
    }
    get length() {
      return this.#files.length;
    }
    item(index) {
      requireArgs(arguments, 1, "'item' on 'FileList'");
      return this.#files[index >>> 0] || null;
    }
    [Symbol.iterator]() {
      return this.#files.values();
    }
  }
  tag(FileList, 'FileList');

  function makeFileList(files) {
    return new FileList(FILE_LIST_KEY, files);
  }

  // FormData. https://xhr.spec.whatwg.org/#interface-formdata

  let formEntries;

  // The value of a FormData entry: a string, or a File.
  function entryValue(value, filename) {
    if (!(value instanceof Blob)) return usv(value);
    if (value instanceof File && filename === undefined) return value;
    const name = filename !== undefined ? usv(filename) : value instanceof File ? value.name : 'blob';
    return new File([value], name, {
      type: value.type,
      lastModified: value instanceof File ? value.lastModified : undefined,
    });
  }

  class FormData {
    #entries = [];
    constructor(form, submitter) {
      if (form === undefined) return;
      // The DOM bindings provide the entries of an HTMLFormElement.
      const entries = internals.formDataEntries ? internals.formDataEntries(form, submitter) : null;
      if (entries === null) {
        throw new TypeError("Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
      }
      for (const [name, value] of entries) {
        // A file input's entry is an empty file: none can be chosen.
        const file = value === null ? new File([], '', { type: 'application/octet-stream' }) : entryValue(value);
        this.#entries.push([usv(name), file]);
      }
    }
    static {
      formEntries = (formData) => formData.#entries;
    }
    append(name, value, filename) {
      requireArgs(arguments, 2, "execute 'append' on 'FormData'");
      this.#entries.push([usv(name), entryValue(value, filename)]);
    }
    delete(name) {
      requireArgs(arguments, 1, "execute 'delete' on 'FormData'");
      name = usv(name);
      this.#entries = this.#entries.filter((entry) => entry[0] !== name);
    }
    get(name) {
      requireArgs(arguments, 1, "execute 'get' on 'FormData'");
      name = usv(name);
      const entry = this.#entries.find((e) => e[0] === name);
      return entry === undefined ? null : entry[1];
    }
    getAll(name) {
      requireArgs(arguments, 1, "execute 'getAll' on 'FormData'");
      name = usv(name);
      return this.#entries.filter((e) => e[0] === name).map((e) => e[1]);
    }
    has(name) {
      requireArgs(arguments, 1, "execute 'has' on 'FormData'");
      name = usv(name);
      return this.#entries.some((e) => e[0] === name);
    }
    set(name, value, filename) {
      requireArgs(arguments, 2, "execute 'set' on 'FormData'");
      name = usv(name);
      const entry = [name, entryValue(value, filename)];
      const i = this.#entries.findIndex((e) => e[0] === name);
      if (i < 0) {
        this.#entries.push(entry);
      } else {
        this.#entries[i] = entry;
        this.#entries = this.#entries.filter((e, j) => j <= i || e[0] !== name);
      }
    }
    forEach(callback, thisArg) {
      requireArgs(arguments, 1, "execute 'forEach' on 'FormData'");
      for (const [name, value] of this.#entries.slice()) {
        ReflectApply(callback, thisArg, [value, name, this]);
      }
    }
    *entries() {
      for (const [name, value] of this.#entries.slice()) yield [name, value];
    }
    *keys() {
      for (const entry of this.#entries.slice()) yield entry[0];
    }
    *values() {
      for (const entry of this.#entries.slice()) yield entry[1];
    }
    [Symbol.iterator]() {
      return this.entries();
    }
  }
  tag(FormData, 'FormData');

  function randomBoundary() {
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    let out = '----WebKitFormBoundary';
    for (let i = 0; i < 16; i++) out += chars[Math.floor(Math.random() * chars.length)];
    return out;
  }

  // https://html.spec.whatwg.org/#multipart-form-data
  function encodeMultipart(formData) {
    const boundary = randomBoundary();
    const escape = (s) => s.replace(/\n/g, '%0A').replace(/\r/g, '%0D').replace(/"/g, '%22');
    const chunks = [];
    for (const [name, value] of formEntries(formData)) {
      let head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' +
        escape(name.replace(/\r\n?|\n/g, '\r\n')) + '"';
      if (value instanceof File) {
        head += '; filename="' + escape(value.name) + '"\r\nContent-Type: ' +
          (value.type || 'application/octet-stream') + '\r\n\r\n';
        chunks.push(encode(head), blobBytes(value), encode('\r\n'));
      } else {
        chunks.push(encode(head + '\r\n\r\n' + value.replace(/\r\n?|\n/g, '\r\n') + '\r\n'));
      }
    }
    chunks.push(encode('--' + boundary + '--\r\n'));
    return { bytes: concat(chunks), type: 'multipart/form-data; boundary=' + boundary };
  }

  // https://url.spec.whatwg.org/#concept-urlencoded-parser
  function parseUrlencoded(text) {
    const formData = new FormData();
    for (const sequence of text.split('&')) {
      if (sequence === '') continue;
      const eq = sequence.indexOf('=');
      const name = eq < 0 ? sequence : sequence.slice(0, eq);
      const value = eq < 0 ? '' : sequence.slice(eq + 1);
      formData.append(percentDecode(name), percentDecode(value));
    }
    return formData;
  }

  function percentDecode(s) {
    const bytes = encode(s.replace(/\+/g, ' '));
    const out = [];
    for (let i = 0; i < bytes.length; i++) {
      const b = bytes[i];
      if (b === 0x25 && i + 2 < bytes.length) {
        const n = parseInt(String.fromCharCode(bytes[i + 1], bytes[i + 2]), 16);
        if (/^[0-9A-Fa-f]{2}$/.test(String.fromCharCode(bytes[i + 1], bytes[i + 2]))) {
          out.push(n);
          i += 2;
          continue;
        }
      }
      out.push(b);
    }
    return decode(new U8(out));
  }

  // Bodies. https://fetch.spec.whatwg.org/#bodyinit-unions
  //
  // A body state is { bytes, pending, used, stream }: `bytes` once known
  // (null for a null body), or `pending`, a promise of them while a
  // response body is still arriving.

  // "Extract a body": the bytes and default Content-Type of a BodyInit.
  function extractBody(body) {
    if (typeof body === 'string') {
      return { bytes: encode(body), type: 'text/plain;charset=UTF-8' };
    }
    const URLSearchParamsRef = globalThis.URLSearchParams;
    if (typeof URLSearchParamsRef === 'function' && body instanceof URLSearchParamsRef) {
      return { bytes: encode(String(body)), type: 'application/x-www-form-urlencoded;charset=UTF-8' };
    }
    if (body instanceof Blob) {
      return { bytes: blobBytes(body), type: body.type === '' ? null : body.type };
    }
    if (isBufferSource(body)) return { bytes: copyBytes(body), type: null };
    if (body instanceof FormData) return encodeMultipart(body);
    if (internals.isReadableStream(body)) {
      throw new TypeError('Streaming request bodies are not supported.');
    }
    return { bytes: encode(String(body)), type: 'text/plain;charset=UTF-8' };
  }

  function bodyState(bytes) {
    return { bytes, pending: null, used: false, stream: null };
  }

  function cloneBody(s) {
    return { bytes: s.bytes, pending: s.pending, used: false, stream: null };
  }

  function bodyUnusable(s) {
    return s.used || (s.stream !== null && s.stream.locked);
  }

  // Takes the body for reading: marks it used and returns a promise of its
  // bytes. The body's own stream reads it while holding the lock.
  function takeBody(s, what, fromStream = false) {
    if (s.bytes === null && s.pending === null) return PromiseRef.resolve(EMPTY);
    if (fromStream ? s.used : bodyUnusable(s)) {
      return PromiseRef.reject(new TypeError("Failed to execute '" + what + "': body stream already read"));
    }
    s.used = true;
    return s.pending !== null ? s.pending : PromiseRef.resolve(s.bytes);
  }

  function mimeType(headers) {
    const type = headers.get('content-type');
    return type === null ? '' : blobType(type);
  }

  // Defines the Body mixin's methods on a class whose instances have body
  // state `stateOf(instance)`.
  function defineBodyMethods(Class, name, stateOf) {
    const methods = {
      arrayBuffer() {
        return then(takeBody(stateOf(this).body, 'arrayBuffer' + "' on '" + name),
          (bytes) => copyBytes(bytes).buffer);
      },
      blob() {
        const s = stateOf(this);
        return then(takeBody(s.body, 'blob' + "' on '" + name),
          (bytes) => makeBlob(bytes, mimeType(s.headers)));
      },
      bytes() {
        return then(takeBody(stateOf(this).body, 'bytes' + "' on '" + name), copyBytes);
      },
      formData() {
        const s = stateOf(this);
        return then(takeBody(s.body, 'formData' + "' on '" + name), (bytes) => {
          const type = mimeType(s.headers);
          if (type.startsWith('application/x-www-form-urlencoded')) {
            return parseUrlencoded(decode(bytes));
          }
          throw new TypeError("Failed to execute 'formData' on '" + name + "': Failed to parse body as FormData.");
        });
      },
      json() {
        return then(takeBody(stateOf(this).body, 'json' + "' on '" + name),
          (bytes) => JSONParse(decode(bytes)));
      },
      text() {
        return then(takeBody(stateOf(this).body, 'text' + "' on '" + name), decode);
      },
    };
    for (const key of Object.keys(methods)) {
      defineProperty(Class.prototype, key, {
        value: methods[key],
        writable: true,
        configurable: true,
        enumerable: true,
      });
    }
    defineProperty(Class.prototype, 'bodyUsed', {
      get() {
        return stateOf(this).body.used;
      },
      configurable: true,
      enumerable: true,
    });
    defineProperty(Class.prototype, 'body', {
      get() {
        const body = stateOf(this).body;
        if (body.bytes === null && body.pending === null) return null;
        if (body.stream === null) {
          body.stream = makeStream(() => takeBody(body, 'read', true), () => {
            body.used = true;
          });
        }
        return body.stream;
      },
      configurable: true,
      enumerable: true,
    });
  }

  // A ReadableStream (streams.js) of one chunk: the bytes `take()`
  // promises.
  function makeStream(take, cancel) {
    return internals.streams().bytesStream(take, cancel);
  }

  // Request. https://fetch.spec.whatwg.org/#request-class

  const STATE_KEY = Symbol('state');
  let requestState;

  const REQUEST_MODES = ['navigate', 'same-origin', 'no-cors', 'cors'];
  const CREDENTIALS = ['omit', 'same-origin', 'include'];
  const CACHE_MODES = ['default', 'no-store', 'reload', 'no-cache', 'force-cache', 'only-if-cached'];
  const REDIRECTS = ['follow', 'error', 'manual'];
  const REFERRER_POLICIES = ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin',
    'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'];

  function enumValue(value, allowed, fallback, what, type) {
    if (value === undefined) return fallback;
    value = String(value);
    if (!allowed.includes(value)) {
      throw new TypeError('Failed to ' + what + ": The provided value '" + value +
        "' is not a valid enum value of type " + type + '.');
    }
    return value;
  }

  class Request {
    #s;
    constructor(input, init) {
      if (input === STATE_KEY) {
        this.#s = init;
        return;
      }
      const what = "construct 'Request'";
      requireArgs(arguments, 1, what);
      const opts = dictionary(init, what);
      const base = input instanceof Request ? input.#s : null;

      let url;
      if (base === null) {
        url = parseURL(input);
        if (url === null) throw new TypeError('Failed to ' + what + ': Invalid URL');
        if (/^[a-z][a-z0-9+.-]*:\/\/[^/?#]*@/i.test(url)) {
          throw new TypeError('Failed to ' + what + ': Request cannot be constructed from a URL that includes credentials: ' + url);
        }
      } else {
        url = base.url;
      }

      let method = base === null ? 'GET' : base.method;
      if (opts.method !== undefined) {
        method = normalizeMethod(opts.method);
        if (method === null) throw new TypeError('Failed to ' + what + ": '" + opts.method + "' is not a valid HTTP method.");
        if (method === undefined) throw new TypeError('Failed to ' + what + ": '" + opts.method + "' HTTP method is unsupported.");
      }

      let referrer = base === null ? 'about:client' : base.referrer;
      if (opts.referrer !== undefined) {
        const value = String(opts.referrer);
        if (value === '') {
          referrer = '';
        } else {
          const parsed = parseURL(value);
          if (parsed === null) throw new TypeError('Failed to ' + what + ": Referrer '" + value + "' is not a valid URL.");
          referrer = parsed;
        }
      }

      const mode = enumValue(opts.mode, REQUEST_MODES, base === null ? 'cors' : base.mode, what, 'RequestMode');
      if (mode === 'navigate') {
        throw new TypeError('Failed to ' + what + ": Cannot construct a Request with a RequestInit whose mode member is set as 'navigate'.");
      }

      const headers = new Headers(opts.headers !== undefined ? opts.headers
        : base !== null ? base.headers : undefined);
      setGuard(headers, 'request');
      if (opts.headers !== undefined) {
        // Re-append through the guard so forbidden headers are dropped.
        const list = headerList(headers).slice();
        headerList(headers).length = 0;
        for (const [name, value] of list) headers.append(name, value);
      }

      let body = bodyState(null);
      const inputBody = base !== null ? base.body : null;
      if ((opts.body !== undefined && opts.body !== null) || (inputBody !== null && (inputBody.bytes !== null || inputBody.pending !== null))) {
        if (method === 'GET' || method === 'HEAD') {
          throw new TypeError('Failed to ' + what + ': Request with GET/HEAD method cannot have body.');
        }
      }
      if (opts.body !== undefined && opts.body !== null) {
        const extracted = extractBody(opts.body);
        body = bodyState(extracted.bytes);
        if (extracted.type !== null && !headers.has('content-type')) {
          headerList(headers).push(['content-type', extracted.type]);
        }
      } else if (inputBody !== null && (inputBody.bytes !== null || inputBody.pending !== null)) {
        if (bodyUnusable(inputBody)) throw new TypeError('Failed to ' + what + ': Cannot construct a Request with a Request object that has already been used.');
        body = cloneBody(inputBody);
        // The new request takes over the input's body.
        inputBody.used = true;
      }

      let signal = base !== null ? base.signal : null;
      if (opts.signal !== undefined) {
        signal = opts.signal;
        if (signal !== null && !isSignal(signal)) {
          throw new TypeError('Failed to ' + what + ": Failed to read the 'signal' property from 'RequestInit': Failed to convert value to 'AbortSignal'.");
        }
      }

      this.#s = {
        method,
        url,
        headers,
        body,
        referrer,
        referrerPolicy: enumValue(opts.referrerPolicy, REFERRER_POLICIES, base === null ? '' : base.referrerPolicy, what, 'ReferrerPolicy'),
        mode,
        credentials: enumValue(opts.credentials, CREDENTIALS, base === null ? 'same-origin' : base.credentials, what, 'RequestCredentials'),
        cache: enumValue(opts.cache, CACHE_MODES, base === null ? 'default' : base.cache, what, 'RequestCache'),
        redirect: enumValue(opts.redirect, REDIRECTS, base === null ? 'follow' : base.redirect, what, 'RequestRedirect'),
        integrity: opts.integrity !== undefined ? String(opts.integrity) : base === null ? '' : base.integrity,
        keepalive: opts.keepalive !== undefined ? !!opts.keepalive : base !== null && base.keepalive,
        // The request's own signal follows the given one.
        signal: signal === null ? null : AbortSignalRef.any([signal]),
      };
    }
    static {
      requestState = (request) => request.#s;
    }
    get method() { return this.#s.method; }
    get url() { return this.#s.url; }
    get headers() { return this.#s.headers; }
    get destination() { return ''; }
    get referrer() { return this.#s.referrer; }
    get referrerPolicy() { return this.#s.referrerPolicy; }
    get mode() { return this.#s.mode; }
    get credentials() { return this.#s.credentials; }
    get cache() { return this.#s.cache; }
    get redirect() { return this.#s.redirect; }
    get integrity() { return this.#s.integrity; }
    get keepalive() { return this.#s.keepalive; }
    get isReloadNavigation() { return false; }
    get isHistoryNavigation() { return false; }
    get duplex() { return 'half'; }
    get signal() {
      const s = this.#s;
      if (s.signal === null) s.signal = new AbortControllerRef().signal;
      return s.signal;
    }
    clone() {
      const s = this.#s;
      if (bodyUnusable(s.body)) {
        throw new TypeError("Failed to execute 'clone' on 'Request': Request body is already used");
      }
      const headers = new Headers(s.headers);
      setGuard(headers, 'request');
      return new Request(STATE_KEY, {
        ...s,
        headers,
        body: cloneBody(s.body),
        signal: s.signal === null ? null : AbortSignalRef.any([s.signal]),
      });
    }
  }
  tag(Request, 'Request');
  defineBodyMethods(Request, 'Request', (request) => requestState(request));

  // Response. https://fetch.spec.whatwg.org/#response-class

  let responseState;
  const NULL_BODY_STATUSES = [101, 103, 204, 205, 304];
  const REDIRECT_STATUSES = [301, 302, 303, 307, 308];

  function responseInit(s, init, what) {
    const opts = dictionary(init, what);
    const status = opts.status === undefined ? 200 : Number(opts.status) & 0xFFFF;
    if (status < 200 || status > 599) {
      throw new RangeError('Failed to ' + what + ": The status provided (" + status + ') is outside the range [200, 599].');
    }
    const statusText = opts.statusText === undefined ? '' : String(opts.statusText);
    if (!/^[\t\x20-\x7E\x80-\xFF]*$/.test(statusText)) {
      throw new TypeError('Failed to ' + what + ': Invalid statusText');
    }
    s.status = status;
    s.statusText = statusText;
    if (opts.headers !== undefined) fillHeaders(s.headers, opts.headers, what);
  }

  class Response {
    #s;
    constructor(body = null, init = undefined) {
      if (body === STATE_KEY) {
        this.#s = init;
        return;
      }
      const what = "construct 'Response'";
      const headers = new Headers();
      setGuard(headers, 'response');
      const s = {
        type: 'default',
        url: '',
        redirected: false,
        status: 200,
        statusText: '',
        headers,
        body: bodyState(null),
      };
      responseInit(s, init, what);
      if (body !== null && body !== undefined) {
        if (NULL_BODY_STATUSES.includes(s.status)) {
          throw new TypeError('Failed to ' + what + ': Response with null body status cannot have body');
        }
        if (internals.isReadableStream(body)) {
          // Read whole, like every other body here; `response.body` is a
          // new stream over the result.
          if (internals.streams().locked(body) || internals.streams().disturbed(body)) {
            throw new TypeError('Failed to ' + what + ': Response body object should not be disturbed or locked');
          }
          const pending = internals.streams().readAllBytes(body);
          ReflectApply(PromiseCatch, pending, [noop]);
          s.body = { bytes: null, pending, used: false, stream: null };
          this.#s = s;
          return;
        }
        const extracted = extractBody(body);
        s.body = bodyState(extracted.bytes);
        if (extracted.type !== null && !headers.has('content-type')) {
          headerList(headers).push(['content-type', extracted.type]);
        }
      }
      this.#s = s;
    }
    static {
      responseState = (response) => response.#s;
    }
    static error() {
      return makeResponse({
        type: 'error',
        url: '',
        redirected: false,
        status: 0,
        statusText: '',
        headers: headersFromFlat([], 'immutable'),
        body: bodyState(null),
      });
    }
    static redirect(url, status = 302) {
      requireArgs(arguments, 1, "execute 'redirect' on 'Response'");
      const parsed = parseURL(url);
      if (parsed === null) throw new TypeError("Failed to execute 'redirect' on 'Response': Invalid URL");
      status = Number(status) & 0xFFFF;
      if (!REDIRECT_STATUSES.includes(status)) {
        throw new RangeError("Failed to execute 'redirect' on 'Response': Invalid status code");
      }
      return makeResponse({
        type: 'default',
        url: '',
        redirected: false,
        status,
        statusText: '',
        headers: headersFromFlat(['location', parsed], 'immutable'),
        body: bodyState(null),
      });
    }
    static json(data, init) {
      requireArgs(arguments, 1, "execute 'json' on 'Response'");
      const text = JSONStringify(data);
      if (text === undefined) {
        throw new TypeError("Failed to execute 'json' on 'Response': The data is not JSON serializable");
      }
      const response = new Response(text, init);
      const headers = responseState(response).headers;
      const list = headerList(headers);
      const i = list.findIndex((pair) => pair[0] === 'content-type');
      if (i >= 0 && list[i][1] === 'text/plain;charset=UTF-8') list[i] = ['content-type', 'application/json'];
      return response;
    }
    get type() { return this.#s.type; }
    get url() { return this.#s.url; }
    get redirected() { return this.#s.redirected; }
    get status() { return this.#s.status; }
    get ok() { return this.#s.status >= 200 && this.#s.status <= 299; }
    get statusText() { return this.#s.statusText; }
    get headers() { return this.#s.headers; }
    clone() {
      const s = this.#s;
      if (bodyUnusable(s.body)) {
        throw new TypeError("Failed to execute 'clone' on 'Response': Response body is already used");
      }
      const headers = new Headers(s.headers);
      setGuard(headers, s.type === 'default' ? 'response' : 'immutable');
      return makeResponse({ ...s, headers, body: cloneBody(s.body) });
    }
  }
  tag(Response, 'Response');
  defineBodyMethods(Response, 'Response', (response) => responseState(response));

  function makeResponse(state) {
    return new Response(STATE_KEY, state);
  }

  // Network requests. Results come back from Rust through the hooks below,
  // keyed by request id; `jobs` holds the requests in progress.

  const jobs = new Map();

  // Where the request's Referer comes from: the document's URL unless the
  // request says otherwise.
  function referrerFor(referrer, policy) {
    if (policy === 'no-referrer' || referrer === '') return undefined;
    if (referrer === 'about:client') {
      const url = documentURL();
      return /^https?:/i.test(url) ? url : undefined;
    }
    return referrer;
  }

  // The request as `native.netSend` takes it.
  function wireRequest(method, url, headerPairs, bytes, credentials, redirect, kind, timeout, referrer) {
    return {
      method,
      url,
      headers: flatHeaders(headerPairs),
      body: bytes === null ? undefined : bytes,
      credentials,
      redirect,
      kind,
      timeout,
      referrer,
      document: documentURL(),
    };
  }

  // Starts a network request. `job` gets `onResponse(head)`,
  // `onBody(bytes)` and `onError(kind)` calls, and `id` once started.
  // Returns the id, which `cancelRequest` takes.
  function startRequest(wire, job) {
    const id = netSend(wire);
    job.id = id;
    jobs.set(id, job);
    return id;
  }

  function cancelRequest(id) {
    if (jobs.delete(id)) netAbort(id);
  }

  function fetch(input, init = undefined) {
    let request;
    try {
      request = new Request(input, init);
    } catch (error) {
      return PromiseRef.reject(error);
    }
    const s = requestState(request);
    const result = new PromiseRef((resolve, reject) => {
      const signal = s.signal;
      if (signal !== null && signal.aborted) {
        reject(signal.reason);
        return;
      }
      let bytes = null;
      if (s.body.bytes !== null || s.body.pending !== null) {
        // Bodies built by Request are always complete.
        bytes = s.body.bytes;
        s.body.used = true;
      }
      const job = {
        response: null,
        unhook: noop,
        onResponse(head) {
          let type = head.type;
          let status = head.status;
          let statusText = head.statusText;
          let flat = head.headers;
          let hasBody = !NULL_BODY_STATUSES.includes(status) && s.method !== 'HEAD';
          let url = stripFragment(head.url);
          if (s.redirect === 'manual' && REDIRECT_STATUSES.includes(status)) {
            type = 'opaqueredirect';
            status = 0;
            statusText = '';
            flat = [];
            hasBody = false;
            url = '';
          }
          const body = bodyState(null);
          if (hasBody) {
            job.body = deferred();
            body.pending = job.body.promise;
          }
          job.state = body;
          job.response = makeResponse({
            type,
            url,
            redirected: head.redirected,
            status,
            statusText,
            headers: headersFromFlat(flat, 'immutable'),
            body,
          });
          resolve(job.response);
        },
        onBody(bytes) {
          job.unhook();
          if (job.body !== undefined) {
            job.state.bytes = bytes;
            job.state.pending = null;
            job.body.resolve(bytes);
          }
        },
        onError() {
          job.unhook();
          const error = new TypeError('Failed to fetch');
          if (job.response === null) {
            reject(error);
          } else if (job.body !== undefined) {
            job.body.reject(error);
          }
        },
      };
      try {
        startRequest(wireRequest(s.method, s.url, headerList(s.headers), bytes, s.credentials,
          s.redirect, 'fetch', 0, referrerFor(s.referrer, s.referrerPolicy)), job);
      } catch (error) {
        reject(error);
        return;
      }
      if (signal !== null) {
        job.unhook = signalAddAlgorithm(signal, (reason) => {
          cancelRequest(job.id);
          if (job.response === null) {
            reject(reason);
          } else if (job.body !== undefined) {
            job.body.reject(reason);
          }
        });
      }
    });
    return result;
  }

  native.netSetHooks({
    onResponse(id, head) {
      const job = jobs.get(id);
      if (job !== undefined) job.onResponse(head);
    },
    onBody(id, buffer) {
      const job = jobs.get(id);
      if (job === undefined) return;
      jobs.delete(id);
      job.onBody(new U8(buffer));
    },
    onError(id, kind) {
      const job = jobs.get(id);
      if (job === undefined) return;
      jobs.delete(id);
      job.onError(kind);
    },
  });

  internals.expose({
    Headers,
    Blob,
    File,
    FileList,
    FormData,
    Request,
    Response,
    fetch,
  });

  Object.assign(internals, {
    Blob,
    File,
    FormData,
    makeBlob,
    makeFileList,
    blobBytes,
    extractBody,
    decodeUtf8: decode,
    encodeUtf8: encode,
    parseURL,
    normalizeMethod,
    isToken,
    normalizeHeaderValue: normalizeValue,
    isValidHeaderValue: isValidValue,
    isForbiddenRequestHeader,
    isForbiddenResponseHeader,
    stripFragment,
    referrerFor,
    wireRequest,
    startRequest,
    cancelRequest,
  });
})
