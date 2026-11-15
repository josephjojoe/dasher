// XMLHttpRequest, on the same network host functions as fetch().
// https://xhr.spec.whatwg.org/
//
// Bodies arrive whole, so a request moves through HEADERS_RECEIVED,
// LOADING and DONE with one `progress` event in between. Synchronous
// requests block the page until the response is complete.
(function (native, internals) {
  'use strict';

  const U8 = Uint8Array;
  const JSONParse = JSON.parse;
  const defineProperty = Object.defineProperty;
  const EventTargetRef = globalThis.EventTarget;
  const netSendSync = native.netSendSync;
  const {
    domException,
    fire,
    defineEventHandlers,
    eventClass,
    extractBody,
    makeBlob,
    decodeUtf8,
    parseURL,
    normalizeMethod,
    isToken,
    normalizeHeaderValue,
    isValidHeaderValue,
    isForbiddenRequestHeader,
    stripFragment,
    referrerFor,
    wireRequest,
    startRequest,
    cancelRequest,
  } = internals;

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to ' + what + ': ' + n + ' argument' + (n === 1 ? '' : 's') +
        ' required, but only ' + args.length + ' present.');
    }
  }

  const ProgressEvent = eventClass('ProgressEvent', { lengthComputable: false, loaded: 0, total: 0 });

  const UNSENT = 0;
  const OPENED = 1;
  const HEADERS_RECEIVED = 2;
  const LOADING = 3;
  const DONE = 4;

  const KEY = Symbol('XMLHttpRequest');
  const RESPONSE_TYPES = ['', 'arraybuffer', 'blob', 'document', 'json', 'text'];

  class XMLHttpRequestEventTarget extends EventTargetRef {
    constructor(key) {
      super();
      if (key !== KEY) throw new TypeError('Illegal constructor');
    }
  }
  tag(XMLHttpRequestEventTarget, 'XMLHttpRequestEventTarget');
  defineEventHandlers(XMLHttpRequestEventTarget.prototype, [
    'onloadstart', 'onprogress', 'onabort', 'onerror', 'onload', 'ontimeout', 'onloadend',
  ]);

  class XMLHttpRequestUpload extends XMLHttpRequestEventTarget {
    constructor(key) {
      super(key);
    }
  }
  tag(XMLHttpRequestUpload, 'XMLHttpRequestUpload');

  function progress(target, type, loaded, total) {
    fire(target, type, { lengthComputable: total > 0, loaded, total }, ProgressEvent);
  }

  class XMLHttpRequest extends XMLHttpRequestEventTarget {
    #x = {
      readyState: UNSENT,
      method: 'GET',
      url: null,
      async: true,
      headers: [],
      send: false,
      // The running request: { id }, or null.
      job: null,
      responseType: '',
      mimeOverride: null,
      timeout: 0,
      withCredentials: false,
      upload: null,
      uploadComplete: true,
      // The response: the head once headers arrive, the bytes once the
      // body has, and `error` for a network error.
      head: null,
      bytes: null,
      error: false,
      cached: undefined,
      text: null,
    };

    constructor() {
      super(KEY);
      this.#x.upload = new XMLHttpRequestUpload(KEY);
    }

    get readyState() {
      return this.#x.readyState;
    }

    get upload() {
      return this.#x.upload;
    }

    open(method, url, async = true, username = undefined, password = undefined) {
      requireArgs(arguments, 2, "execute 'open' on 'XMLHttpRequest'");
      const x = this.#x;
      const normalized = normalizeMethod(method);
      if (normalized === null) {
        throw domException('SyntaxError', "Failed to execute 'open' on 'XMLHttpRequest': '" + method + "' is not a valid HTTP method.");
      }
      if (normalized === undefined) {
        throw domException('SecurityError', "Failed to execute 'open' on 'XMLHttpRequest': '" + method + "' HTTP method is unsupported.");
      }
      const parsed = parseURL(url);
      if (parsed === null) {
        throw domException('SyntaxError', "Failed to execute 'open' on 'XMLHttpRequest': Invalid URL");
      }
      async = arguments.length < 3 ? true : !!async;
      if (!async && (x.timeout !== 0 || x.responseType !== '')) {
        throw domException('InvalidAccessError', "Failed to execute 'open' on 'XMLHttpRequest': Synchronous requests from a document must not set a response type or a timeout.");
      }
      void username;
      void password;
      this.#terminate();
      x.method = normalized;
      x.url = parsed;
      x.async = async;
      x.headers = [];
      x.send = false;
      this.#reset();
      if (x.readyState !== OPENED) {
        x.readyState = OPENED;
        fire(this, 'readystatechange');
      }
    }

    setRequestHeader(name, value) {
      requireArgs(arguments, 2, "execute 'setRequestHeader' on 'XMLHttpRequest'");
      const x = this.#x;
      if (x.readyState !== OPENED || x.send) {
        throw domException('InvalidStateError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED.");
      }
      name = String(name);
      value = normalizeHeaderValue(value);
      if (!isToken(name)) {
        throw domException('SyntaxError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + name + "' is not a valid HTTP header field name.");
      }
      if (!isValidHeaderValue(value)) {
        throw domException('SyntaxError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + value + "' is not a valid HTTP header field value.");
      }
      const lower = name.toLowerCase();
      if (isForbiddenRequestHeader(lower)) return;
      const existing = x.headers.find((pair) => pair[0] === lower);
      if (existing !== undefined) {
        existing[1] += ', ' + value;
      } else {
        x.headers.push([lower, value]);
      }
    }

    get timeout() {
      return this.#x.timeout;
    }
    set timeout(value) {
      const x = this.#x;
      if (x.readyState === OPENED && !x.async) {
        throw domException('InvalidAccessError', "Failed to set the 'timeout' property on 'XMLHttpRequest': Timeouts cannot be set for synchronous requests made from a document.");
      }
      x.timeout = Number(value) >>> 0;
    }

    get withCredentials() {
      return this.#x.withCredentials;
    }
    set withCredentials(value) {
      const x = this.#x;
      if ((x.readyState !== UNSENT && x.readyState !== OPENED) || x.send) {
        throw domException('InvalidStateError', "Failed to set the 'withCredentials' property on 'XMLHttpRequest': The value may only be set if the object's state is UNSENT or OPENED.");
      }
      x.withCredentials = !!value;
    }

    send(body = null) {
      const x = this.#x;
      if (x.readyState !== OPENED || x.send) {
        throw domException('InvalidStateError', "Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.");
      }
      if (x.method === 'GET' || x.method === 'HEAD') body = null;
      let bytes = null;
      const headers = x.headers.slice();
      if (body !== null && body !== undefined) {
        const extracted = extractBody(body);
        bytes = extracted.bytes;
        if (extracted.type !== null && !headers.some((pair) => pair[0] === 'content-type')) {
          headers.push(['content-type', extracted.type]);
        }
      }
      x.uploadComplete = bytes === null;
      x.send = true;
      const wire = wireRequest(x.method, x.url, headers, bytes,
        x.withCredentials ? 'include' : 'same-origin', 'follow', 'xhr', x.timeout,
        referrerFor('about:client', ''));

      if (!x.async) {
        this.#sendSync(wire);
        return;
      }

      progress(this, 'loadstart', 0, 0);
      if (!x.uploadComplete) progress(x.upload, 'loadstart', 0, bytes.length);
      // A listener may have aborted or reopened the request.
      if (x.readyState !== OPENED || !x.send) return;

      const uploaded = bytes === null ? 0 : bytes.length;
      const job = {
        onResponse: (head) => {
          if (x.job !== job) return;
          this.#uploadDone(uploaded);
          x.head = head;
          x.readyState = HEADERS_RECEIVED;
          fire(this, 'readystatechange');
        },
        onBody: (bytes) => {
          if (x.job !== job) return;
          x.job = null;
          this.#receive(bytes);
        },
        onError: (kind) => {
          if (x.job !== job) return;
          x.job = null;
          this.#requestError(kind === 'timeout' && x.timeout !== 0 ? 'timeout' : 'error');
        },
      };
      x.job = job;
      startRequest(wire, job);
    }

    #sendSync(wire) {
      const x = this.#x;
      const result = netSendSync(wire);
      if (result.error !== undefined) {
        this.#requestError('error');
        throw domException('NetworkError', "Failed to execute 'send' on 'XMLHttpRequest': Failed to load '" + x.url + "'.");
      }
      x.head = result.head;
      this.#receive(new U8(result.body));
    }

    // The response body has arrived: LOADING, then DONE.
    #receive(bytes) {
      const x = this.#x;
      x.bytes = bytes;
      x.cached = undefined;
      x.text = null;
      const length = this.getResponseHeader('content-length');
      const total = length !== null && /^\d+$/.test(length) ? Number(length) : 0;
      if (x.async) {
        x.readyState = LOADING;
        fire(this, 'readystatechange');
        if (x.readyState !== LOADING) return;
        progress(this, 'progress', bytes.length, total);
        if (x.readyState !== LOADING) return;
      }
      x.readyState = DONE;
      x.send = false;
      fire(this, 'readystatechange');
      progress(this, 'load', bytes.length, total);
      progress(this, 'loadend', bytes.length, total);
    }

    // Upload finished once the response starts arriving.
    #uploadDone(length) {
      const x = this.#x;
      if (x.uploadComplete) return;
      x.uploadComplete = true;
      progress(x.upload, 'progress', length, length);
      progress(x.upload, 'load', length, length);
      progress(x.upload, 'loadend', length, length);
    }

    // https://xhr.spec.whatwg.org/#request-error-steps
    #requestError(event) {
      const x = this.#x;
      x.readyState = DONE;
      x.send = false;
      x.head = null;
      x.bytes = null;
      x.error = true;
      x.cached = undefined;
      x.text = null;
      if (!x.async) return;
      fire(this, 'readystatechange');
      if (!x.uploadComplete) {
        x.uploadComplete = true;
        progress(x.upload, event, 0, 0);
        progress(x.upload, 'loadend', 0, 0);
      }
      progress(this, event, 0, 0);
      progress(this, 'loadend', 0, 0);
    }

    #terminate() {
      const x = this.#x;
      if (x.job !== null) {
        cancelRequest(x.job.id);
        x.job = null;
      }
    }

    #reset() {
      const x = this.#x;
      x.head = null;
      x.bytes = null;
      x.error = false;
      x.cached = undefined;
      x.text = null;
    }

    abort() {
      const x = this.#x;
      this.#terminate();
      if ((x.readyState === OPENED && x.send) || x.readyState === HEADERS_RECEIVED ||
          x.readyState === LOADING) {
        this.#requestError('abort');
      }
      if (x.readyState === DONE) {
        x.readyState = UNSENT;
        this.#reset();
        x.error = true;
      }
    }

    get responseURL() {
      const head = this.#x.head;
      return head === null ? '' : stripFragment(head.url);
    }

    get status() {
      const head = this.#x.head;
      return head === null ? 0 : head.status;
    }

    get statusText() {
      const head = this.#x.head;
      return head === null ? '' : head.statusText;
    }

    getResponseHeader(name) {
      requireArgs(arguments, 1, "execute 'getResponseHeader' on 'XMLHttpRequest'");
      const head = this.#x.head;
      if (head === null) return null;
      const lower = String(name).toLowerCase();
      const flat = head.headers;
      let value = null;
      for (let i = 0; i + 1 < flat.length; i += 2) {
        if (flat[i] === lower) value = value === null ? flat[i + 1] : value + ', ' + flat[i + 1];
      }
      return value;
    }

    getAllResponseHeaders() {
      const head = this.#x.head;
      if (head === null) return '';
      const flat = head.headers;
      const names = [];
      for (let i = 0; i < flat.length; i += 2) if (!names.includes(flat[i])) names.push(flat[i]);
      names.sort();
      let out = '';
      for (const name of names) out += name + ': ' + this.getResponseHeader(name) + '\r\n';
      return out;
    }

    overrideMimeType(mime) {
      requireArgs(arguments, 1, "execute 'overrideMimeType' on 'XMLHttpRequest'");
      const x = this.#x;
      if (x.readyState === LOADING || x.readyState === DONE) {
        throw domException('InvalidStateError', "Failed to execute 'overrideMimeType' on 'XMLHttpRequest': MimeType cannot be overridden when the state is LOADING or DONE.");
      }
      x.mimeOverride = String(mime);
    }

    get responseType() {
      return this.#x.responseType;
    }
    set responseType(value) {
      const x = this.#x;
      value = String(value);
      if (!RESPONSE_TYPES.includes(value)) return;
      if (x.readyState === LOADING || x.readyState === DONE) {
        throw domException('InvalidStateError', "Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be set if the object's state is LOADING or DONE.");
      }
      if (x.readyState === OPENED && !x.async) {
        throw domException('InvalidAccessError', "Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be changed for synchronous requests made from a document.");
      }
      x.responseType = value;
    }

    #mimeType() {
      const x = this.#x;
      if (x.mimeOverride !== null) return x.mimeOverride.toLowerCase();
      const type = this.getResponseHeader('content-type');
      return type === null ? '' : type.toLowerCase();
    }

    get responseText() {
      const x = this.#x;
      if (x.responseType !== '' && x.responseType !== 'text') {
        throw domException('InvalidStateError', "Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text' (was '" + x.responseType + "').");
      }
      if ((x.readyState !== LOADING && x.readyState !== DONE) || x.bytes === null) return '';
      if (x.text === null) x.text = decodeUtf8(x.bytes);
      return x.text;
    }

    get responseXML() {
      const x = this.#x;
      if (x.responseType !== '' && x.responseType !== 'document') {
        throw domException('InvalidStateError', "Failed to read the 'responseXML' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'document' (was '" + x.responseType + "').");
      }
      return this.#document();
    }

    // A document parsed from the response, when the DOM bindings provide
    // `internals.parseDocument(text, mimeType, url)`. XML responses are
    // parsed for responseXML and responseType 'document', HTML ones only
    // for the latter, as the standard says.
    #document() {
      const x = this.#x;
      if (x.readyState !== DONE || x.bytes === null) return null;
      if (x.cached === undefined) {
        const parse = internals.parseDocument;
        // Without a Content-Type, a response counts as text/xml.
        const essence = this.#mimeType().split(';')[0].trim() || 'text/xml';
        const html = essence === 'text/html';
        const xml = essence === 'text/xml' || essence === 'application/xml' || essence.endsWith('+xml');
        x.cached = typeof parse === 'function' && (xml || (html && x.responseType === 'document'))
          ? parse(decodeUtf8(x.bytes), essence, this.responseURL)
          : null;
      }
      return x.cached;
    }

    get response() {
      const x = this.#x;
      if (x.responseType === '' || x.responseType === 'text') return this.responseText;
      if (x.readyState !== DONE || x.bytes === null) return null;
      if (x.cached !== undefined) return x.cached;
      switch (x.responseType) {
        case 'arraybuffer':
          x.cached = x.bytes.slice().buffer;
          break;
        case 'blob':
          x.cached = makeBlob(x.bytes, this.#mimeType());
          break;
        case 'json':
          try {
            x.cached = JSONParse(decodeUtf8(x.bytes));
          } catch (_) {
            x.cached = null;
          }
          break;
        case 'document':
          return this.#document();
      }
      return x.cached;
    }
  }
  tag(XMLHttpRequest, 'XMLHttpRequest');
  defineEventHandlers(XMLHttpRequest.prototype, ['onreadystatechange']);
  for (const [name, value] of [
    ['UNSENT', UNSENT],
    ['OPENED', OPENED],
    ['HEADERS_RECEIVED', HEADERS_RECEIVED],
    ['LOADING', LOADING],
    ['DONE', DONE],
  ]) {
    defineProperty(XMLHttpRequest, name, { value, enumerable: true });
    defineProperty(XMLHttpRequest.prototype, name, { value, enumerable: true });
  }

  internals.expose({
    ProgressEvent,
    XMLHttpRequestEventTarget,
    XMLHttpRequestUpload,
    XMLHttpRequest,
  });
})
