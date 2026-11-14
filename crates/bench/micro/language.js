// Language micro-benchmarks: small fixed loops over one kind of operation.
//
// `dasher-bench micro` loads this file in a page and runs one benchmark per
// page load; `node micro/node.js` runs the same loops under Node (with
// `--jitless` for the fair comparison). Each entry is
// `bench(name, iterations, fn)`: `fn(n)` does the operation `n` times and
// may return a promise. Results go to `sink`, so nothing is computed for
// nobody. Keep the loop bodies fixed: changing one makes old numbers
// meaningless. The iteration count is a starting point: the harness raises
// it for loops that take under 100 ms, which leaves ns per op unchanged.

var MICRO = typeof MICRO === "undefined" ? [] : MICRO;
var sink;
function bench(name, n, fn) {
  MICRO.push({ suite: "language", name: name, n: n, fn: fn });
}

bench("function call", 2e6, function (n) {
  function add(a, b) {
    return a + b;
  }
  let s = 0;
  for (let i = 0; i < n; i++) s = add(s, i) | 0;
  sink = s;
});

bench("method call", 2e6, function (n) {
  const o = {
    v: 1,
    get(x) {
      return this.v + x;
    },
  };
  let s = 0;
  for (let i = 0; i < n; i++) s = o.get(s) | 0;
  sink = s;
});

bench("create+call closure", 1e6, function (n) {
  let s = 0;
  for (let i = 0; i < n; i++) {
    const f = (x) => x + i;
    s = f(s) | 0;
  }
  sink = s;
});

bench("integer loop", 5e6, function (n) {
  let s = 0;
  for (let i = 0; i < n; i++) s = (s + i * 3) | 0;
  sink = s;
});

bench("property read/write, one shape", 2e6, function (n) {
  const o = { a: 1, b: 2, c: 3 };
  for (let i = 0; i < n; i++) o.a = (o.b + o.c + i) | 0;
  sink = o.a;
});

bench("property read/write, four shapes", 2e6, function (n) {
  const objs = [
    { a: 1, b: 2 },
    { b: 2, a: 1 },
    { a: 1, b: 2, c: 3 },
    { x: 0, a: 1, b: 2 },
  ];
  for (let i = 0; i < n; i++) {
    const o = objs[i & 3];
    o.a = (o.b + i) | 0;
  }
  sink = objs[0].a;
});

bench("property read/write, 64 shapes", 2e6, function (n) {
  const objs = [];
  for (let k = 0; k < 64; k++) {
    const o = {};
    o["p" + k] = k;
    o.a = 1;
    o.b = 2;
    objs.push(o);
  }
  for (let i = 0; i < n; i++) {
    const o = objs[i & 63];
    o.a = (o.b + i) | 0;
  }
  sink = objs[0].a;
});

bench("object literal", 5e5, function (n) {
  for (let i = 0; i < n; i++) sink = { a: i, b: 2, c: "x", d: null };
});

bench("spread and Object.assign", 2e5, function (n) {
  const base = { a: 1, b: 2, c: 3 };
  for (let i = 0; i < n; i++) {
    const x = { ...base, d: i };
    sink = Object.assign({}, x, { e: 1 });
  }
});

bench("destructuring and rest/spread", 1e5, function (n) {
  const arr = [1, 2, 3, 4];
  const obj = { a: 1, b: 2, c: 3 };
  function three(x, y, z) {
    return x + y + z;
  }
  let s = 0;
  for (let i = 0; i < n; i++) {
    const [p, ...rest] = arr;
    const { a, ...others } = obj;
    s = (s + p + a + others.b + three(...rest)) | 0;
  }
  sink = s;
});

bench("apply and arguments", 5e5, function (n) {
  function sum() {
    let s = 0;
    for (let j = 0; j < arguments.length; j++) s += arguments[j];
    return s;
  }
  const args = [1, 2, 3];
  let s = 0;
  for (let i = 0; i < n; i++) s = (s + sum.apply(null, args)) | 0;
  sink = s;
});

bench("map/filter/reduce", 5e4, function (n) {
  const a = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
  for (let i = 0; i < n; i++) {
    sink = a
      .map((x) => x * 2)
      .filter((x) => x > 4)
      .reduce((p, x) => p + x, 0);
  }
});

bench("concat.apply", 2e5, function (n) {
  const parts = [[1, 2], [3], [4, 5, 6], [7]];
  for (let i = 0; i < n; i++) sink = [].concat.apply([], parts);
});

bench("string concat", 1e6, function (n) {
  let s = "";
  for (let i = 0; i < n; i++) {
    s += "ab" + (i & 7);
    if (s.length > 1000) s = "";
  }
  sink = s;
});

bench("string methods", 1e5, function (n) {
  const s = "  The quick brown fox jumps over the lazy dog  ";
  let x = 0;
  for (let i = 0; i < n; i++) {
    x += s.indexOf("lazy") + s.slice(6, 11).toUpperCase().length;
    x += s.trim().split(" ").length + s.replace("fox", "cat").length;
    x += s.includes("dog") ? 1 : 0;
  }
  sink = x;
});

bench("regexps", 2.5e4, function (n) {
  const re = /(\d+)-(\w+)/;
  const g = /o/g;
  const t = /^\s*[a-z]+\s*$/i;
  let x = 0;
  for (let i = 0; i < n; i++) {
    x += re.exec("item 123-abc")[1].length;
    x += "foo boo zoo".replace(g, "0").length;
    x += t.test("  Hello ") ? 1 : 0;
  }
  sink = x;
});

// Patterns and calls taken from recorded real pages (DAS-38): user-agent
// sniffing, type checks, CSS name conversion, trimming, class lists,
// email checks and cookie lookups, each on the short strings pages give
// them. One op is the twelve calls.
bench("regexps, page patterns", 1e4, function (n) {
  const ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
  const types = ["[object Object]", "[object Array]", "[object Function]"];
  const mobile = /(iPhone|iPod|iPad).*AppleWebKit(?!.*Safari)/i;
  const chrome = /Chrome\/(\d+)\.(\d+)/;
  const bot = /bot|crawl|spider|slurp/i;
  const type = /\[object (Boolean|Number|String|Function|Array|Date|RegExp)\]/;
  const upper = /[A-Z]/g;
  const vendor = /^(webkit|moz|ms|o)-/;
  const trim = /^[\s﻿\xA0]+|[\s﻿\xA0]+$/g;
  const space = /\s+/;
  const email = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
  let x = 0;
  for (let i = 0; i < n; i++) {
    x += mobile.test(ua) ? 1 : 0;
    x += chrome.exec(ua)[1].length;
    x += bot.test(ua) ? 1 : 0;
    const m = type.exec(types[i % 3]);
    x += m ? m[1].length : 0;
    x += "backgroundColor".replace(upper, "-$&").length;
    x += vendor.test("transform") ? 1 : 0;
    x += "  some text  ".replace(trim, "").length;
    x += "btn btn-primary  active".split(space).length;
    x += email.test("first.last@example.com") ? 1 : 0;
    x += /(^|; )session=([^;]*)/.exec("a=1; session=abc; b=2")[2].length;
    x += new RegExp("(^|; )" + "theme" + "=([^;]*)").test("a=1; theme=dark") ? 1 : 0;
    x += "a-b-c".split("-").length;
  }
  sink = x;
});

// Searches over page text, as tracking scripts run them on timers: an
// email address, a list of keywords and runs of whitespace over 13 KB
// with no address and no keyword in it. One op is the three searches.
bench("regexps over page text", 20, function (n) {
  const words = "the quick brown fox jumps over the lazy dog lorem ipsum dolor sit amet consectetur adipiscing elit".split(" ");
  let text = "";
  for (let i = 0; i < 2000; i++) text += words[(i * 7) % words.length] + (i % 13 == 0 ? ".\n" : " ");
  text += "’";
  const email = /[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+/g;
  const keywords = /\b(?:casino|viagra|lottery|bitcoin|crypto|forex|payday|loans?)\b/gi;
  const space = /\s+/g;
  let x = 0;
  for (let i = 0; i < n; i++) {
    x += (text.match(email) || []).length;
    x += (text.match(keywords) || []).length;
    x += text.replace(space, " ").length;
  }
  sink = x;
});

bench("JSON", 3e4, function (n) {
  const o = { id: 1, name: "widget", tags: ["a", "b"], nested: { v: true, n: null, f: 1.5 } };
  for (let i = 0; i < n; i++) sink = JSON.parse(JSON.stringify(o));
});

bench("Map/Set", 5e5, function (n) {
  const m = new Map();
  const set = new Set();
  let x = 0;
  for (let i = 0; i < n; i++) {
    const k = i & 1023;
    m.set(k, i);
    set.add(k);
    x = (x + m.get(k) + (set.has(k) ? 1 : 0)) | 0;
  }
  sink = x;
});

bench("class instantiation", 5e5, function (n) {
  class A {
    constructor(x) {
      this.x = x;
    }
  }
  class B extends A {
    constructor(x) {
      super(x);
      this.y = 1;
    }
  }
  for (let i = 0; i < n; i++) sink = new B(i);
});

bench("promise chain", 1e5, function (n) {
  let p = Promise.resolve(0);
  for (let i = 0; i < n; i++) p = p.then((x) => x + 1);
  return p.then((x) => {
    sink = x;
  });
});

bench("async/await", 2e5, async function (n) {
  async function f(x) {
    return x + 1;
  }
  let s = 0;
  for (let i = 0; i < n; i++) s = await f(s);
  sink = s;
});

bench("generators", 2e6, function (n) {
  function* g(n) {
    for (let i = 0; i < n; i++) yield i;
  }
  let s = 0;
  for (const v of g(n)) s = (s + v) | 0;
  sink = s;
});

bench("Date and Intl formatting", 3e3, function (n) {
  const d = new Date(Date.UTC(2026, 10, 13, 12, 30));
  const nf = new Intl.NumberFormat("en-US");
  const df = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" });
  let x = 0;
  for (let i = 0; i < n; i++) {
    x += d.toLocaleDateString("en-US").length + nf.format(i * 1.5).length;
    x += d.toISOString().length + df.format(d).length;
  }
  sink = x;
});

bench("try/catch", 1e6, function (n) {
  let s = 0;
  for (let i = 0; i < n; i++) {
    try {
      if (i % 2 === 0) throw new Error("x");
      s++;
    } catch (e) {
      s--;
    }
  }
  sink = s;
});

bench("for-in and Object.keys", 1e5, function (n) {
  const o = { a: 1, b: 2, c: 3, d: 4, e: 5 };
  let s = 0;
  for (let i = 0; i < n; i++) {
    for (const k in o) s += o[k];
    s += Object.keys(o).length;
  }
  sink = s;
});

bench("URL parsing", 5e4, function (n) {
  for (let i = 0; i < n; i++) {
    sink = new URL("../page?q=" + i + "#top", "https://example.com/a/b/c").href;
  }
});

bench("new Function", 3e4, function (n) {
  let s = 0;
  for (let i = 0; i < n; i++) s += new Function("a", "b", "return a + b + " + i)(1, 2);
  sink = s;
});
