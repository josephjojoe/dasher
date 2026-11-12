// Constructs the parser can only tell apart by reading ahead to the
// matching bracket. Every bracket here is long enough for the engine to
// remember what it found (see vendor/rquickjs-sys/PATCHES.md), and the
// whole file is inside one, so the inner ones are answered from memory.
var lookahead = (function () {
  var r = {};
  // A bracketed expression against an arrow function.
  r.paren = (1 + [10, 20, 30].length + "a string literal that is long enough to be remembered".length * 0);
  r.arrow = ((first, second = [1, 2, 3].length, { third = "a default that is long enough to be remembered" } = {}) => first + second + third.length)(1);
  r.newline = typeof ((function () { return "a function body long enough to be remembered by the table"; })
    );
  // An object or array literal against a destructuring assignment.
  var a, b, rest;
  r.literal = { alpha: [1, 2, 3], beta: { gamma: "a value that is long enough to be remembered by the table" } }.alpha.length;
  ({ alpha: a, beta: { gamma: b = "a default that is long enough to be remembered by the table" }, ...rest } = { alpha: 1, beta: {}, delta: 4 });
  r.object = [a, b.length, rest.delta];
  [a, [b, ...rest] = ["an array default that is long enough to be remembered by the table"]] = [7, [8, 9, 10]];
  r.array = [a, b, rest];
  // An '=' in a parameter list gives the parameters a scope of their own,
  // also when the '=' is inside a template literal.
  var x = "outer";
  function params(p = `${(x = "set by a default, inside a template that is long enough")}`, q = () => x) { var x = "inner"; return [p.length, q()]; }
  r.params = params();
  function plain(p, q, /* a comment that makes this list long enough to be remembered by the table */ s) { var p = "inner"; return arguments[0]; }
  r.plain = plain("argument");
  var y = "outer";
  function keyed({ [`${(y = "key")}`]: z }, /* a comment that makes this list long enough to be remembered */ unused) { var y = "inner"; return z; }
  r.keyed = [keyed({ key: 1 }), y];
  // A for-of loop over a destructuring pattern.
  r.forof = [];
  for ({ key: a, value: b = "a default that is long enough to be remembered by the table" } of [{ key: 1 }, { key: 2, value: "v" }]) r.forof.push(a + ":" + b.length);
  // The same in strict code, in a generator and in an async function,
  // where 'yield' and 'await' are read differently.
  r.strict = (function () { "use strict"; return ((u, v = { w: "a default in strict code that is long enough to be remembered" }) => u + v.w.length)(1); })();
  r.generator = [...(function* () { yield ((m, n = [1, 2, 3, "a default that is long enough to be remembered by the table"]) => m + n.length)(1); var await = 2, re = [await /2/ 1, "padding that makes this bracket long enough to be remembered"]; yield re[0]; })()];
  r.async = String(async function () { return [await /2/.test("2"), "padding that makes this bracket long enough to be remembered"]; }).length;
  return JSON.stringify(r);
})();
