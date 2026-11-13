// URL and URLSearchParams. Parsing and setters run in Rust (the `url`
// crate, see urls.rs): a URL holds the parsed record, and its getters read
// one component off it. This file holds the WebIDL surface and the live
// link between a URL and its searchParams.
// https://url.spec.whatwg.org/#api
(function (native, internals) {
  'use strict';

  const usv = internals.usv;
  const { urlNew, urlGet, urlSetPart, urlParse, urlHref, urlSet } = native;
  const queryParse = native.queryParse;
  const querySerialize = native.querySerialize;
  const ReflectOwnKeys = Reflect.ownKeys;
  const propertyIsEnumerable = Object.prototype.propertyIsEnumerable;

  // Indexes into the component arrays Rust returns.
  const HREF = 0;
  const ORIGIN = 1;
  const PROTOCOL = 2;
  const USERNAME = 3;
  const PASSWORD = 4;
  const HOST = 5;
  const HOSTNAME = 6;
  const PORT = 7;
  const PATHNAME = 8;
  const SEARCH = 9;
  const HASH = 10;

  function tag(ctor, name) {
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  // A value as a string for Rust, which makes it a USVString itself.
  const str = (value) => typeof value === 'string' ? value : String(value);
  const strOr = (value) => value === undefined ? undefined : str(value);

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to ' + what + ': ' + n + ' argument' + (n === 1 ? '' : 's') +
        ' required, but only ' + args.length + ' present.');
    }
  }

  function parsePairs(query) {
    const flat = queryParse(query);
    const list = [];
    for (let i = 0; i < flat.length; i += 2) list.push([flat[i], flat[i + 1]]);
    return list;
  }

  function serializePairs(list) {
    const flat = [];
    for (const [name, value] of list) flat.push(name, value);
    return querySerialize(flat);
  }

  // Wired up inside the class bodies, which own the private fields.
  let linkParams;
  let resetParams;
  let setUrlQuery;

  class URLSearchParams {
    #list = [];
    #url = null;

    constructor(init = '') {
      if ((typeof init === 'object' && init !== null) || typeof init === 'function') {
        if (#list in init) {
          this.#list = init.#list.map((pair) => [pair[0], pair[1]]);
        } else if (typeof init[Symbol.iterator] === 'function') {
          for (const pair of init) {
            const items = typeof pair === 'string' ? null : [...pair];
            if (items === null || items.length !== 2) {
              throw new TypeError("Failed to construct 'URLSearchParams': The provided value cannot be converted to a sequence.");
            }
            this.#list.push([usv(items[0]), usv(items[1])]);
          }
        } else {
          for (const key of ReflectOwnKeys(init)) {
            if (typeof key === 'string' && propertyIsEnumerable.call(init, key)) {
              this.#list.push([usv(key), usv(init[key])]);
            }
          }
        }
      } else {
        let query = usv(init);
        if (query.startsWith('?')) query = query.slice(1);
        this.#list = parsePairs(query);
      }
    }

    static {
      linkParams = (params, url) => {
        params.#url = url;
      };
      resetParams = (params, search) => {
        params.#list = parsePairs(search.startsWith('?') ? search.slice(1) : search);
      };
    }

    #update() {
      if (this.#url !== null) setUrlQuery(this.#url, serializePairs(this.#list));
    }

    get size() {
      return this.#list.length;
    }

    append(name, value) {
      requireArgs(arguments, 2, "execute 'append' on 'URLSearchParams'");
      this.#list.push([usv(name), usv(value)]);
      this.#update();
    }

    delete(name, value) {
      requireArgs(arguments, 1, "execute 'delete' on 'URLSearchParams'");
      name = usv(name);
      if (value === undefined) {
        this.#list = this.#list.filter((pair) => pair[0] !== name);
      } else {
        value = usv(value);
        this.#list = this.#list.filter((pair) => pair[0] !== name || pair[1] !== value);
      }
      this.#update();
    }

    get(name) {
      requireArgs(arguments, 1, "execute 'get' on 'URLSearchParams'");
      name = usv(name);
      const pair = this.#list.find((p) => p[0] === name);
      return pair === undefined ? null : pair[1];
    }

    getAll(name) {
      requireArgs(arguments, 1, "execute 'getAll' on 'URLSearchParams'");
      name = usv(name);
      return this.#list.filter((p) => p[0] === name).map((p) => p[1]);
    }

    has(name, value) {
      requireArgs(arguments, 1, "execute 'has' on 'URLSearchParams'");
      name = usv(name);
      if (value === undefined) return this.#list.some((p) => p[0] === name);
      value = usv(value);
      return this.#list.some((p) => p[0] === name && p[1] === value);
    }

    set(name, value) {
      requireArgs(arguments, 2, "execute 'set' on 'URLSearchParams'");
      name = usv(name);
      value = usv(value);
      const i = this.#list.findIndex((p) => p[0] === name);
      if (i < 0) {
        this.#list.push([name, value]);
      } else {
        this.#list[i][1] = value;
        this.#list = this.#list.filter((p, j) => j <= i || p[0] !== name);
      }
      this.#update();
    }

    sort() {
      // Array sort is stable; string comparison is by UTF-16 code units.
      this.#list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      this.#update();
    }

    forEach(callback, thisArg) {
      if (typeof callback !== 'function') {
        throw new TypeError("Failed to execute 'forEach' on 'URLSearchParams': The callback provided as parameter 1 is not a function.");
      }
      for (let i = 0; i < this.#list.length; i++) {
        const [name, value] = this.#list[i];
        callback.call(thisArg, value, name, this);
      }
    }

    *entries() {
      for (let i = 0; i < this.#list.length; i++) yield [this.#list[i][0], this.#list[i][1]];
    }

    *keys() {
      for (let i = 0; i < this.#list.length; i++) yield this.#list[i][0];
    }

    *values() {
      for (let i = 0; i < this.#list.length; i++) yield this.#list[i][1];
    }

    toString() {
      return serializePairs(this.#list);
    }
  }
  Object.defineProperty(URLSearchParams.prototype, Symbol.iterator, {
    value: URLSearchParams.prototype.entries,
    writable: true,
    configurable: true,
  });
  tag(URLSearchParams, 'URLSearchParams');

  class URL {
    #u;
    #query = null;

    constructor(url, base) {
      requireArgs(arguments, 1, "construct 'URL'");
      const u = urlNew(str(url), strOr(base));
      if (u === undefined) {
        throw new TypeError("Failed to construct 'URL': Invalid URL");
      }
      this.#u = u;
    }

    static {
      setUrlQuery = (url, serialized) => {
        urlSetPart(url.#u, 'search', serialized);
      };
    }

    static canParse(url, base) {
      requireArgs(arguments, 1, "execute 'canParse' on 'URL'");
      return urlNew(str(url), strOr(base)) !== undefined;
    }

    static parse(url, base) {
      requireArgs(arguments, 1, "execute 'parse' on 'URL'");
      const href = urlHref(str(url), strOr(base));
      return href === undefined ? null : new URL(href);
    }

    #set(part, value) {
      if (!urlSetPart(this.#u, part, str(value))) {
        throw new TypeError("Failed to set the '" + part + "' property on 'URL': Invalid URL");
      }
      if (this.#query !== null && (part === 'href' || part === 'search')) {
        resetParams(this.#query, urlGet(this.#u, SEARCH));
      }
    }

    get href() { return urlGet(this.#u, HREF); }
    set href(value) { this.#set('href', value); }
    get origin() { return urlGet(this.#u, ORIGIN); }
    get protocol() { return urlGet(this.#u, PROTOCOL); }
    set protocol(value) { this.#set('protocol', value); }
    get username() { return urlGet(this.#u, USERNAME); }
    set username(value) { this.#set('username', value); }
    get password() { return urlGet(this.#u, PASSWORD); }
    set password(value) { this.#set('password', value); }
    get host() { return urlGet(this.#u, HOST); }
    set host(value) { this.#set('host', value); }
    get hostname() { return urlGet(this.#u, HOSTNAME); }
    set hostname(value) { this.#set('hostname', value); }
    get port() { return urlGet(this.#u, PORT); }
    set port(value) { this.#set('port', value); }
    get pathname() { return urlGet(this.#u, PATHNAME); }
    set pathname(value) { this.#set('pathname', value); }
    get search() { return urlGet(this.#u, SEARCH); }
    set search(value) { this.#set('search', value); }
    get hash() { return urlGet(this.#u, HASH); }
    set hash(value) { this.#set('hash', value); }

    get searchParams() {
      if (this.#query === null) {
        const params = new URLSearchParams(urlGet(this.#u, SEARCH));
        linkParams(params, this);
        this.#query = params;
      }
      return this.#query;
    }

    toString() {
      return urlGet(this.#u, HREF);
    }

    toJSON() {
      return urlGet(this.#u, HREF);
    }
  }
  tag(URL, 'URL');

  internals.expose({ URL, URLSearchParams });
  // Parses `input` against `base`, returning the component array (see the
  // indexes above) or undefined. Shared with location.
  internals.urlParse = (input, base) => urlParse(str(input), strOr(base));
  // The serialization alone, or undefined.
  internals.urlHref = (input, base) => urlHref(str(input), strOr(base));
  internals.urlSet = (href, part, value) => urlSet(href, part, usv(value));
  internals.URL_PARTS = { HREF, ORIGIN, PROTOCOL, USERNAME, PASSWORD, HOST, HOSTNAME, PORT, PATHNAME, SEARCH, HASH };
})
