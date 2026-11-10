// The `crypto` global: getRandomValues and randomUUID. `crypto.subtle` is
// a stub defined in stubs.js.
// https://w3c.github.io/webcrypto/#crypto-interface
(function (native, internals) {
  'use strict';

  const domException = internals.domException;
  const randomBytes = native.randomBytes;
  const typedArrayTag = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const MAX_BYTES = 65536;
  const INTEGER_ARRAYS = new Set([
    'Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
    'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array',
  ]);
  const KEY = Symbol('Crypto');

  class Crypto {
    constructor(key) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
    }

    getRandomValues(array) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': 1 argument required, but only 0 present.");
      }
      const name = ArrayBuffer.isView(array) ? typedArrayTag.call(array) : undefined;
      if (name === undefined) {
        throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.");
      }
      if (!INTEGER_ARRAYS.has(name)) {
        throw domException('TypeMismatchError', "Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" + name + "', which is not an integer array type.");
      }
      const length = array.byteLength;
      if (length > MAX_BYTES) {
        throw domException('QuotaExceededError', "Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (" + length + ') exceeds the number of bytes of entropy available via this API (65536).');
      }
      new Uint8Array(array.buffer, array.byteOffset, length).set(randomBytes(length));
      return array;
    }

    randomUUID() {
      const b = randomBytes(16);
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      let hex = '';
      for (let i = 0; i < 16; i++) {
        if (i === 4 || i === 6 || i === 8 || i === 10) hex += '-';
        hex += (b[i] < 16 ? '0' : '') + b[i].toString(16);
      }
      return hex;
    }
  }
  Object.defineProperty(Crypto.prototype, Symbol.toStringTag, { value: 'Crypto', configurable: true });

  const crypto = new Crypto(KEY);
  internals.expose({ Crypto });
  Object.defineProperty(globalThis, 'crypto', {
    get() {
      return crypto;
    },
    configurable: true,
    enumerable: true,
  });
})
