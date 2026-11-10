// localStorage and sessionStorage, kept in memory by the host per origin.
// https://html.spec.whatwg.org/#storage
//
// Items are also reachable as properties (`localStorage.token`), through a
// Proxy that mimics WebIDL named properties: prototype members such as
// `getItem` and `length` win on reads, and string-keyed writes store items.
(function (native, internals) {
  'use strict';

  const usv = internals.usv;
  const domException = internals.domException;
  const ReflectGet = Reflect.get;
  const ReflectSet = Reflect.set;
  const ReflectHas = Reflect.has;
  const ReflectOwnKeys = Reflect.ownKeys;
  const ReflectDefineProperty = Reflect.defineProperty;
  const ReflectDeleteProperty = Reflect.deleteProperty;
  const ReflectGetOwnPropertyDescriptor = Reflect.getOwnPropertyDescriptor;
  const KEY = Symbol('Storage');
  const areaOf = new WeakMap();

  function area(storage) {
    const id = areaOf.get(storage);
    if (id === undefined) throw new TypeError('Illegal invocation');
    return id;
  }

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError("Failed to execute '" + what + "' on 'Storage': " + n + ' argument' +
        (n === 1 ? '' : 's') + ' required, but only ' + args.length + ' present.');
    }
  }

  function setItem(id, key, value) {
    if (!native.storageSet(id, key, value)) {
      throw domException('QuotaExceededError',
        "Failed to execute 'setItem' on 'Storage': Setting the value of '" + key + "' exceeded the quota.");
    }
  }

  class Storage {
    constructor(key) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
    }
    get length() {
      return native.storageLength(area(this));
    }
    key(index) {
      requireArgs(arguments, 1, 'key');
      const result = native.storageKey(area(this), index >>> 0);
      return result === undefined ? null : result;
    }
    getItem(key) {
      requireArgs(arguments, 1, 'getItem');
      const value = native.storageGet(area(this), usv(key));
      return value === undefined ? null : value;
    }
    setItem(key, value) {
      requireArgs(arguments, 2, 'setItem');
      setItem(area(this), usv(key), usv(value));
    }
    removeItem(key) {
      requireArgs(arguments, 1, 'removeItem');
      native.storageRemove(area(this), usv(key));
    }
    clear() {
      native.storageClear(area(this));
    }
  }
  Object.defineProperty(Storage.prototype, Symbol.toStringTag, { value: 'Storage', configurable: true });

  function item(id, key) {
    return native.storageGet(id, usv(key));
  }

  function makeStorage(id) {
    const target = new Storage(KEY);
    areaOf.set(target, id);
    const proxy = new Proxy(target, {
      get(t, prop, receiver) {
        if (typeof prop === 'string' && !ReflectHas(t, prop)) {
          const value = item(id, prop);
          if (value !== undefined) return value;
        }
        return ReflectGet(t, prop, receiver === proxy ? t : receiver);
      },
      set(t, prop, value, receiver) {
        if (typeof prop !== 'string') return ReflectSet(t, prop, value, receiver === proxy ? t : receiver);
        setItem(id, usv(prop), usv(value));
        return true;
      },
      has(t, prop) {
        return (typeof prop === 'string' && item(id, prop) !== undefined) || ReflectHas(t, prop);
      },
      deleteProperty(t, prop) {
        if (typeof prop === 'string' && item(id, prop) !== undefined) {
          native.storageRemove(id, usv(prop));
          return true;
        }
        return ReflectDeleteProperty(t, prop);
      },
      ownKeys(t) {
        return [...native.storageKeys(id), ...ReflectOwnKeys(t)];
      },
      getOwnPropertyDescriptor(t, prop) {
        if (typeof prop === 'string') {
          const value = item(id, prop);
          if (value !== undefined) return { value, writable: true, enumerable: true, configurable: true };
        }
        return ReflectGetOwnPropertyDescriptor(t, prop);
      },
      defineProperty(t, prop, desc) {
        if (typeof prop !== 'string') return ReflectDefineProperty(t, prop, desc);
        setItem(id, usv(prop), usv(desc.value));
        return true;
      },
    });
    areaOf.set(proxy, id);
    return proxy;
  }

  const localStorage = makeStorage(0);
  const sessionStorage = makeStorage(1);

  // Storage events fire in other windows sharing the storage; a page here
  // is alone, so the class exists but is never dispatched.
  const StorageEvent = internals.eventClass('StorageEvent', {
    key: null,
    oldValue: null,
    newValue: null,
    url: '',
    storageArea: null,
  });

  internals.expose({ Storage, StorageEvent });
  for (const [name, value] of [['localStorage', localStorage], ['sessionStorage', sessionStorage]]) {
    Object.defineProperty(globalThis, name, { get: () => value, enumerable: true, configurable: true });
  }
})
