// The `crypto` global: getRandomValues and randomUUID, and the digest()
// behind `crypto.subtle`, whose other methods are stubs (stubs.js).
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

  // SubtleCrypto.digest: SHA-1 and the SHA-2 family, resolving with an
  // ArrayBuffer. https://w3c.github.io/webcrypto/#SubtleCrypto-method-digest
  const DIGESTS = new Map([['sha-1', 'SHA-1'], ['sha-256', 'SHA-256'], ['sha-384', 'SHA-384'],
    ['sha-512', 'SHA-512']]);
  internals.digest = function digest(algorithm, data, argc) {
    const what = "Failed to execute 'digest' on 'SubtleCrypto': ";
    try {
      if (argc < 2) {
        throw new TypeError(what + '2 arguments required, but only ' + argc + ' present.');
      }
      let name = algorithm;
      if (algorithm !== null && typeof algorithm === 'object') {
        if (algorithm.name === undefined) {
          throw new TypeError(what + "Failed to read the 'name' property from 'Algorithm': Required member is undefined.");
        }
        name = algorithm.name;
      }
      const normalized = DIGESTS.get(String(name).toLowerCase());
      if (normalized === undefined) {
        throw domException('NotSupportedError', what + 'Algorithm: Unrecognized name');
      }
      let bytes;
      if (data instanceof ArrayBuffer) {
        bytes = new Uint8Array(data);
      } else if (ArrayBuffer.isView(data)) {
        bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      } else {
        throw new TypeError(what + "parameter 2 is not of type '(ArrayBuffer or ArrayBufferView)'.");
      }
      // Hashed now: the spec copies the data before returning.
      return Promise.resolve(native.digest(normalized, bytes));
    } catch (error) {
      return Promise.reject(error);
    }
  };

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
