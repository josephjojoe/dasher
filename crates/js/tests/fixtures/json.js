// JSON.parse and JSON.stringify, compared with what the engine gives
// without dasher's JSON patches (see PATCHES.md): key order, __proto__,
// repeated keys, numbers, lone surrogates, error messages and their
// positions, revivers, toJSON, replacers, gaps, and objects that change
// while they are serialized. See tests/runtime.rs,
// json_is_the_same_however_it_is_parsed_and_written.
(function () {
  const out = [];
  function t(name, f) {
    let r;
    try {
      r = f();
      r = typeof r === "string" ? JSON.stringify(r) : String(r);
    } catch (e) {
      r = "ERR " + e.name + ": " + e.message;
    }
    out.push(name + " => " + r);
  }
  const S = (...a) => JSON.stringify(...a);
  function dump(v) {
    if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v);
    if (typeof v === "string")
      return JSON.stringify(v) + "/" + Array.from(v, (c) => c.charCodeAt(0).toString(16)).join(".");
    if (v === null || typeof v !== "object") return String(v);
    if (Array.isArray(v)) return "[" + v.map(dump).join(",") + "]";
    return "{" + Object.getOwnPropertyNames(v).map((k) => JSON.stringify(k) + ":" + dump(v[k]) +
      (Object.getOwnPropertyDescriptor(v, k).enumerable ? "" : "!")).join(",") + "|" +
      (Object.getPrototypeOf(v) === Object.prototype) + "}";
  }
  const P = (x, r) => dump(JSON.parse(x, r));

  t("lone", () => P('"' + String.fromCharCode(0xd800) + '" '));
  t("lone 2", () => P('"a' + String.fromCharCode(0xdc00) + 'b"'));
  t("escapes", () => P('["\\ud800", "\\ud83d\\ude00", "\\u00e9\\u0041", "\\b\\f\\n\\r\\t\\/\\\\\\""]'));
  t("wide", () => P('{"\u0100": "\u1234\u00e9", "\u00e9": ["\u0101"]}'));
  t("dup", () => P('{"a":1,"b":2,"a":3,"c":{"x":1,"x":[2]}}'));
  t("proto", () => P('{"__proto__":1,"a":{"__proto__":{"b":2}}}'));
  t("int keys", () => P('{"b":1,"2":2,"a":3,"1":4,"4294967294":5,"4294967295":6,"01":7}'));
  t("numbers", () => P('[0,-0,1,-1,2147483647,2147483648,-2147483648,-2147483649,123456789012345,1234567890123456,9007199254740993,1e5,1E-5,1.5,-0.0,0.1,1e400,-1e-400,12345678901234567890123,3.141592653589793238462643383279]'));
  t("minus zero", () => Object.is(JSON.parse("-0"), -0) + "," + Object.is(JSON.parse("[-0]")[0], -0));
  t("shapes", () => P('[{"a":1,"b":2},{"a":3,"b":4},{"a":5},{"a":6,"b":7,"c":8},{"b":9,"a":10},{"a":11,"a":12,"b":13},{"a":14,"b":15}]'));
  t("shapes nested", () => P('[{"a":{"a":1,"x":2},"b":2},{"a":{"a":3},"b":4},{"a":{"a":5,"x":6},"b":7}]'));
  t("shapes churn", () => {
    let r = "";
    for (let i = 0; i < 300; i++) {
      const k = "k" + (i % 7);
      const o = JSON.parse('{"' + k + '":{"' + k + '":' + i + ',"y":[{"' + k + '":1}]},"z":' + i + '}');
      if (i % 50 == 0) r += S(o);
    }
    return r;
  });
  t("white space", () => P(' \t\n\r[ 1 , { "a" : 2 } ] \r\n'));
  t("empty", () => P('[{},[],"",[[]],{"":{}}]'));
  [1, null, true, {toString() { return '[1,{"a":2}]'; }}, [1, 2], [3]].forEach((x, i) => t("non-string " + i, () => P(x)));
  t("reviver", () => { const keys = []; const r = JSON.parse('{"a":[1,{"b":2}],"c":3,"d":{"e":4}}', function (k, v) { keys.push(k + ":" + typeof this); if (k === "c") return undefined; if (k === "e") return v * 10; return v; }); return keys.join(",") + " " + dump(r); });
  t("reviver adds", () => P('{"a":1,"b":2}', function (k, v) { if (k === "a") this.z = 9; return v; }));
  const bad = ['', ' ', '[1,2,', '{"\u00e9":1, x}', '\u2028[1]', '"\\x"', '01', '-', '-a', '1.', '1e', '1e+', '.5', '+1',
    '{"a":1}x', '{"a":1}\0', '"\0"', '"a\nb"', "'a'", '[1,]', '{"a":1,}', '{a:1}', '{"a" 1}', 'tru', 'nul', 'falsey', 'NaN',
    '[1 2]', '{"a":1 "b":2}', '"\\u12"', '"\\u12G4"', '"abc', '["a\u00e9b", x]', '\u00e9', '[' + String.fromCharCode(0xd800) + ']',
    '{"a":[1,{"b":"\u0100",}]}', '[1]\n\n  x'];
  bad.forEach((x, i) => t("bad " + i, () => P(x)));
  t("deep", () => { const n = 1000; return S(JSON.parse("[".repeat(n) + "]".repeat(n))).length; });

  let seed = 1;
  const rnd = (n) => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % n;
  const keys = ["a", "b", "id", "name", "__proto__", "0", "1", "10", "4294967295", "-1", "é", "ሴ",
    "x\\ny", "\\u0041", "", "constructor", "toString", "01", "k" + "z".repeat(70)];
  const strs = ["", "x", "hello world", "é", " ", "\\ud800", "\\udc00x", "\\ud83d\\ude00",
    "\\\"q\\\"", "a\\/b", "\\b\\f\\n\\r\\t", "ÿĀ", "ሴ", String.fromCharCode(0xd800), "z".repeat(100)];
  const nums = ["0", "-0", "1", "-1", "2147483647", "2147483648", "-2147483649", "123456789012345",
    "1234567890123456", "1e5", "1E-5", "1.5", "-0.0", "0.1", "1e400", "12345678901234567890123"];
  const ws = () => ["", " ", "\n", "\t", "\r\n", ""][rnd(6)];
  function gen(d) {
    const r = rnd(d > 4 ? 6 : 10);
    if (r === 0 || r === 3) return nums[rnd(nums.length)];
    if (r === 1 || r === 4 || r === 5) return '"' + strs[rnd(strs.length)] + '"';
    if (r === 2) return ["true", "false", "null"][rnd(3)];
    if (r < 8) {
      const a = [];
      for (let i = rnd(5); i > 0; i--) a.push(gen(d + 1));
      return ws() + "[" + a.join(ws() + "," + ws()) + "]";
    }
    const a = [], base = rnd(3), n = rnd(6);
    for (let i = 0; i < n; i++)
      a.push('"' + (base === 0 ? keys[i] : keys[rnd(keys.length)]) + '"' + ws() + ":" + ws() + gen(d + 1));
    return "{" + a.join(",") + "}";
  }
  const mutations = [(s) => s.slice(0, rnd(s.length + 1)),
    (s) => { const i = rnd(s.length + 1); return s.slice(0, i) + ',}]:"\\0x-.eE'[rnd(14)] + s.slice(i); },
    (s) => s + " x", (s) => s.replace('"', "'"), (s) => s];
  let h = 0, errors = 0;
  for (let i = 0; i < 3000; i++) {
    let text = gen(0);
    if (rnd(3) === 0) text = mutations[rnd(mutations.length)](text);
    let r;
    try {
      r = dump(JSON.parse(text)) + S(JSON.parse(text)) + S(JSON.parse(text), null, rnd(3));
    } catch (e) {
      errors++;
      r = "ERR " + e.name + ": " + e.message;
    }
    for (let j = 0; j < r.length; j++) h = (h * 31 + r.charCodeAt(j)) | 0;
  }
  out.push("parse corpus => " + h + " " + errors);

t("prims", () => S([1, -0, 1.5, 1e21, 1e-7, NaN, Infinity, -Infinity, true, false, null, undefined, "x", 2147483647, -2147483648, 0.1+0.2, 5e-324, 1.7976931348623157e308]));
t("strings", () => S(["\u0000\u001f\u007f\u0080ÿ", "\"\\\/\b\f\n\r\t", "\ud800", "a\udc00b", "😀", "\ud83d", "\ude00\ud83d", "é  ", "Ā\u0001"]));
t("keys", () => S({"\ud800": 1, "a\"b": 2, "\n": 3, "é": 4, "Ā": 5, "": 6}));
t("order", () => S({b: 1, a: 2, 1: 3, 0: 4, "4294967294": 5, "4294967295": 6, "2147483648": 7, "-1": 8, "01": 9, [Symbol("s")]: 10}));
t("nonenum", () => { const o = {a: 1}; Object.defineProperty(o, "h", {value: 2, enumerable: false}); Object.defineProperty(o, "g", {get() { return 3; }, enumerable: true}); o.z = 4; return S(o); });
t("deleted", () => { const o = {a: 1, b: 2, c: 3}; delete o.b; o.d = 4; return S(o); });
t("toJSON", () => S({a: {toJSON(k) { return "k=" + k; }}, b: [ {toJSON(k) { return typeof k + k; }} ], c: new Date(0), d: {toJSON: 5}}));
t("toJSON proto", () => { Object.prototype.toJSON = function (k) { return k === "" ? this : "P" + k; }; try { return S({a: {x: 1}, b: [1, {}]}); } finally { delete Object.prototype.toJSON; } });
t("toJSON getter", () => { let n = 0; Object.defineProperty(Object.prototype, "toJSON", {get() { n++; return undefined; }, configurable: true}); try { return S({a: {b: {}}, c: [{}]}) + n; } finally { delete Object.prototype.toJSON; } });
t("mutate delete", () => { const o = {a: {toJSON() { delete o.b; o.e = 9; return 1; }}, b: 2, c: 3}; return S(o); });
t("mutate proto get", () => { const o = {a: {toJSON() { delete o.b; return 1; }}, b: 2, c: 3}; Object.setPrototypeOf(o, {b: "fromproto"}); return S(o); });
t("mutate value", () => { const o = {a: {toJSON() { o.b = "changed"; return 1; }}, b: 2}; return S(o); });
t("mutate to getter", () => { const o = {a: {toJSON() { Object.defineProperty(o, "b", {get() { return "g"; }, enumerable: true}); return 1; }}, b: 2}; return S(o); });
t("mutate nonenum", () => { const o = {a: {toJSON() { Object.defineProperty(o, "b", {enumerable: false}); return 1; }}, b: 2, c: 3}; return S(o); });
t("array mutate", () => { const a = [1, {toJSON() { a.length = 2; a.push(7); return 0; }}, 3, 4]; return S(a); });
t("array shrink", () => { const a = [1, {toJSON() { a.length = 1; return 0; }}, 3, 4]; Array.prototype[3] = "P"; try { return S(a); } finally { delete Array.prototype[3]; } });
t("holes", () => S([1, , 3, , ]));
t("sparse", () => { const a = []; a[5] = 1; a.x = 2; return S(a); });
t("cycle", () => { const o = {a: {}}; o.a.b = o; return S(o); });
t("cycle arr", () => { const a = [1]; a.push([a]); return S(a); });
t("not cycle", () => { const x = {v: 1}; return S([x, x, {y: x}]); });
t("replacer fn", () => S({a: 1, b: [1, 2, {c: 3}], d: "s"}, function (k, v) { return typeof v === 'number' ? v * 10 + "|" + k + "|" + (this === undefined) : v; }));
t("replacer holder", () => { let h; S({x: 1}, function (k, v) { if (k === "") h = Object.keys(this).join() + ":" + (this[""] === v); return v; }); return h; });
t("replacer arr", () => S({a: 1, b: 2, c: {a: 3, d: 4}, 1: "one"}, ["a", 1, "c", new String("b"), new Number(1), {}, "a"]));
t("gap num", () => S({a: [1, {b: 2}], c: {}, d: []}, null, 4));
t("gap str", () => S({a: [1, {b: 2}], c: "x"}, null, "--->ABCDEFGHIJKLMN"));
t("gap big", () => S([1, [2]], null, 20));
t("gap obj", () => S({a: 1}, null, new String("\t")) + S({a: [1]}, null, new Number(2)));
t("wrappers", () => S([new String("s"), new Number(3), new Boolean(false), Object(1n) instanceof BigInt]));
t("bigint", () => S({a: 1n}));
t("bigint toJSON", () => { BigInt.prototype.toJSON = function () { return this.toString() + "n"; }; try { return S({a: 1n, b: [2n]}); } finally { delete BigInt.prototype.toJSON; } });
t("funcs syms", () => S({f() {}, s: Symbol(), u: undefined, a: [function () {}, Symbol(), undefined]}));
t("top level", () => [S(undefined), S(function () {}), S(Symbol()), S("x"), S(1), S(null)].join("|"));
t("proxy", () => S(new Proxy({a: 1, b: [2]}, { get(t, k, r) { return k === "toJSON" ? undefined : (typeof t[k] === 'number' ? t[k] + 100 : t[k]); }, ownKeys(t) { return ["b", "a"]; } })));
t("class", () => { class A { #p = 1; x = 2; get y() { return 3; } static z = 4; } return S([new A(), A]); });
t("map etc", () => S([new Map([[1, 2]]), new Set([1]), /re/g, new Error("e"), Object.create(null), Object.create({inh: 1})]));
t("nested deep", () => { let o = {}; for (let i = 0; i < 100; i++) o = {o, i}; return S(o).length; });
t("int keys object", () => S({2: "b", 1: "a", x: 1, 0: "z"}));
t("typed", () => S([new Uint8Array([1, 2]), new ArrayBuffer(2), new DataView(new ArrayBuffer(1))]));
t("arguments", () => (function () { return S(arguments); })(1, "a"));
t("string obj keys", () => S(new String("ab")));
t("getter throws", () => S({get a() { throw new Error("boom"); }}));
t("toJSON throws", () => S([{toJSON() { throw new RangeError("tj"); }}]));
t("random", () => {
  let seed = 7; const rnd = n => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % n;
  const ks = ["a", "b", "c", "d", "é", "Ā", "x y", "0", "10"];
  function g(d) { const r = rnd(d > 3 ? 5 : 8); if (r === 0) return rnd(1000) - 500; if (r === 1) return rnd(1000) / 7; if (r === 2) return "s" + "éā\n\"".charAt(rnd(5)) + rnd(9); if (r === 3) return [null, true, false, undefined][rnd(4)]; if (r === 4) return "plain"; if (r < 6) { const a = []; for (let i = rnd(5); i > 0; i--) a.push(g(d + 1)); return a; } const o = {}; for (let i = rnd(5); i > 0; i--) o[ks[rnd(ks.length)]] = g(d + 1); if (rnd(5) === 0) delete o.a; return o; }
  let h = 0, all = "";
  for (let i = 0; i < 3000; i++) { const v = g(0); const s = S(v) + S(v, null, rnd(3)) + S(v, (k, x) => typeof x === "number" ? x + 1 : x); all += s.length + ","; for (let j = 0; j < s.length; j++) h = (h * 31 + s.charCodeAt(j)) | 0; }
  return h + " " + all.length;
});
  return out;
})()
