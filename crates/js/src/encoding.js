// TextEncoder, TextDecoder, atob and btoa.
// https://encoding.spec.whatwg.org/#api
// https://html.spec.whatwg.org/#atob
(function (native, internals) {
  'use strict';

  const usv = internals.usv;
  const domException = internals.domException;
  const Uint8ArrayCtor = Uint8Array;
  const isView = ArrayBuffer.isView;

  function tag(ctor, name) {
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  // Views any BufferSource as bytes, without copying.
  function bytesOf(input, what) {
    if (input === undefined) return new Uint8ArrayCtor(0);
    if (input instanceof ArrayBuffer ||
        (typeof SharedArrayBuffer === 'function' && input instanceof SharedArrayBuffer)) {
      return new Uint8ArrayCtor(input);
    }
    if (isView(input)) return new Uint8ArrayCtor(input.buffer, input.byteOffset, input.byteLength);
    throw new TypeError('Failed to execute ' + what +
      ": The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
  }

  class TextEncoder {
    get encoding() {
      return 'utf-8';
    }
    encode(input = '') {
      return native.utf8Encode(usv(input));
    }
    encodeInto(source, destination) {
      if (arguments.length < 2) {
        throw new TypeError("Failed to execute 'encodeInto' on 'TextEncoder': 2 arguments required, but only " +
          arguments.length + ' present.');
      }
      if (!(destination instanceof Uint8ArrayCtor)) {
        throw new TypeError("Failed to execute 'encodeInto' on 'TextEncoder': parameter 2 is not of type 'Uint8Array'.");
      }
      const [bytes, read] = native.utf8EncodePrefix(usv(source), destination.length);
      destination.set(bytes);
      return { read, written: bytes.length };
    }
  }
  tag(TextEncoder, 'TextEncoder');

  class TextDecoder {
    #encoding;
    #fatal;
    #ignoreBOM;
    #pending = null;
    #bomSeen = false;
    constructor(label = 'utf-8', options) {
      const name = native.encodingName(usv(label));
      if (name === undefined) {
        throw new RangeError("Failed to construct 'TextDecoder': The encoding label provided ('" +
          String(label) + "') is invalid.");
      }
      const init = options === undefined || options === null ? {} : options;
      this.#encoding = name;
      this.#fatal = !!init.fatal;
      this.#ignoreBOM = !!init.ignoreBOM;
    }
    get encoding() {
      return this.#encoding;
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }
    decode(input, options) {
      const stream = !!(options !== undefined && options !== null && options.stream);
      let bytes = bytesOf(input, "'decode' on 'TextDecoder'");
      const pending = this.#pending;
      if (pending !== null) {
        const joined = new Uint8ArrayCtor(pending.length + bytes.length);
        joined.set(pending);
        joined.set(bytes, pending.length);
        bytes = joined;
      }
      const result = native.decode(this.#encoding, bytes, this.#fatal, stream);
      if (result === null) {
        this.#pending = null;
        this.#bomSeen = false;
        throw new TypeError("Failed to execute 'decode' on 'TextDecoder': The encoded data was not valid.");
      }
      let text = result[0];
      const consumed = result[1];
      this.#pending = stream && consumed < bytes.length ? bytes.slice(consumed) : null;
      if (!this.#bomSeen && text.length > 0) {
        this.#bomSeen = true;
        if (!this.#ignoreBOM && this.#encoding !== 'windows-1252' && text.charCodeAt(0) === 0xFEFF) {
          text = text.slice(1);
        }
      }
      if (!stream) this.#bomSeen = false;
      return text;
    }
  }
  tag(TextDecoder, 'TextDecoder');

  function btoa(data) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'btoa' on 'Window': 1 argument required, but only 0 present.");
    }
    const text = String(data);
    if (/[^\u0000-ÿ]/.test(text)) {
      throw domException('InvalidCharacterError',
        "Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.");
    }
    return native.btoa(text);
  }

  function atob(data) {
    if (arguments.length === 0) {
      throw new TypeError("Failed to execute 'atob' on 'Window': 1 argument required, but only 0 present.");
    }
    const decoded = native.atob(usv(data));
    if (decoded === undefined) {
      throw domException('InvalidCharacterError',
        "Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.");
    }
    return decoded;
  }

  internals.expose({ TextEncoder, TextDecoder, btoa, atob });
  internals.bytesOf = bytesOf;
})
