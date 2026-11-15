// Functions here are compiled on their first call, most of them only
// after the script has
// set up what they capture. Each answer must be the one the engine gives
// when it compiles every function with its script.
var out = {};

// Closures over the enclosing functions, a declaration that comes after the
// function using it, shadowing and block scopes.
var topLevel = 'top';
function outer(a) {
  var b = 'b';
  function inner(c) {
    return function innermost() { return [topLevel, a, b, c, later].join(); };
  }
  let later = 'later';
  return inner;
}
out.closures = outer('a')('c')();
var shadow = 'global';
function shadowing() {
  var shadow = 'outer';
  return function () {
    var seen = shadow;
    { let shadow = 'block'; return seen + ':' + shadow; }
  };
}
out.shadowing = shadowing()();
var loops = [];
for (let i = 0; i < 3; i++) loops.push(function () { return i; });
out.loops = loops.map(function (f) { return f(); });
function tdz() {
  function read() { return t; }
  var before;
  try { read(); } catch (e) { before = e.name; }
  let t = 'set';
  return [before, read()];
}
out.tdz = tdz();
function constant() {
  const c = 1;
  return function () { try { c = 2; } catch (e) { return e.name; } };
}
out.constant = constant()();
function inBlock() { { function hoisted() { return seen; } var seen = 'seen'; return hoisted(); } }
out.inBlock = inBlock();
var factorial = function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); };
out.named = [factorial(5), typeof fact];

// 'with' around a function, and in one.
var w = { a: 'from with' }, a = 'global a';
with (w) { var fromWith = function () { return a; }; }
out.withAround = fromWith();
function hasWith(o) { var b = 'local'; return function () { with (o) { return b + c; } }; }
out.withInside = [hasWith({ c: '!' })(), hasWith({ b: 'o', c: '?' })()];

// Direct eval: in the function, in a function inside it, defining a
// variable a function defined earlier reads, in strict code.
function evalLater() {
  function read() { return typeof made === 'undefined' ? 'none' : made; }
  eval('var made = "made"');
  return read();
}
function evalReads() { let q = 'q'; function g() { return q; } return eval('g() + q'); }
function evalInner() { var r = 'r'; return function () { return eval('r'); }; }
function evalStrict() { 'use strict'; var s = 's'; return eval('var x = 1; s + typeof x'); }
var indirect = eval;
function evalIndirect() { var topLevel = 'local'; return indirect('topLevel'); }
out.eval = [evalLater(), evalReads(), evalInner()(), evalStrict(), evalIndirect()];

// arguments, default parameters, rest.
function args() { return function () { return arguments.length; }; }
function argsArrow() { return () => arguments[0]; }
function mapped(x) { arguments[0] = 'changed'; return x; }
function strictArgs(x) { 'use strict'; arguments[0] = 'changed'; return x; }
var fallback = 'outer';
function defaults(p = fallback, q = () => p + fallback) { var fallback = 'inner'; return q() + ':' + fallback; }
function rest(first, ...more) { return () => first + more.length; }
out.args = [args()(1, 2), argsArrow()('A'), mapped('x'), strictArgs('x'), defaults(), rest(1, 2, 3)()];

// Classes: fields, private names, static blocks, super, new.target.
class Base {
  constructor(x) { this.x = x; }
  who() { return 'Base' + this.x; }
  static make() { return new this(1); }
}
class Derived extends Base {
  #secret = 41;
  static #count = 0;
  field = () => this.#secret + 1;
  static { Derived.#count = 10; }
  constructor() { super(2); this.viaArrow = () => super.who(); }
  who() { return 'Derived>' + super.who(); }
  #method() { return 'private method'; }
  get #getter() { return 'private getter'; }
  callPrivate() { return [this.#method(), this.#getter, #secret in this, #secret in {}]; }
  static count() { return Derived.#count; }
}
class Accessors {
  get #f() { return this.f; } set #f(v) { this.f = v; }
  set #s(v) { this.s = v; }
  run() { this.#f = 1; this.#f ??= 2; this.#f += 1; this.#s = 3; return () => [this.#f, this.s]; }
}
var d = new Derived();
out.classes = [d.field(), d.who(), d.viaArrow(), d.callPrivate(), Derived.count(), Base.make().who(),
               new Accessors().run()()];
function Target() { return () => new.target === Target; }
out.newTarget = [new Target()(), Target()()];
var proto = { hi() { return 'proto'; } };
var obj = { __proto__: proto, hi() { return 'obj>' + super.hi(); }, get g() { return super.hi(); }, set s(v) { this.v = v; } };
obj.s = 'set';
out.objects = [obj.hi(), obj.g, obj.v];

// Strict mode from around a function and from its own directive.
function strictOuter() { 'use strict'; return function () { return this === undefined; }; }
function sloppy() { return function () { return typeof this; }; }
function ownDirective() { 'use strict'; return (function () { return this; })() === undefined; }
out.strict = [strictOuter()(), sloppy()(), ownDirective()];
// A class is strict code inside a sloppy function, heritage included.
function sloppyAround() {
  class K { m() { return this; } static n() { try { return arguments.callee; } catch (e) { return e.name; } } }
  var D = class extends function () { return typeof this; } {};
  return [K.prototype.m.call(undefined) === undefined, K.n(), Object.getPrototypeOf(D).call(undefined)];
}
out.classStrict = sloppyAround();

// Generators and async functions.
function* gen() { const inner = function* () { yield* [1, 2]; }; yield* inner(); yield 3; }
out.generators = [...gen()];
async function later2() { const f = async () => { await null; return 'async'; }; return await f(); }
later2().then(function (v) { out.async = v; });

// Properties of functions that have not run yet, and their text.
function lengths(a, b, c = 1, d) {}
out.properties = [lengths.length, lengths.name, (async function () {}).constructor.name,
                  (function* () {}).constructor.name, (() => {}).name, String(lengths),
                  String(function  spaced ( x,y ) { /* c */ return x; })];

// Control flow inside a function compiled later.
function control(n) {
  outer: for (var i = 0; i < 3; i++) {
    switch (n) { case 1: continue outer; default: try { return 'try' + i; } finally { n = 0; } }
  }
  return 'done';
}
out.control = [control(1), control(2)];

// Functions called at once, nested.
out.iife = [(function () { var z = 'z'; return (() => z + '!')(); })(), !function () { return 1; }()];

// Function constructor and toString of a function compiled late.
var constructed = new Function('a', 'b', 'return a + b');
out.constructed = [constructed(1, 2), String(constructed)];

// Where a function compiled later reports an error.
function thrower() {
  return function deeper() {
    return [1].map(function each() { throw new Error('here'); });
  };
}
try { thrower()(); } catch (e) { out.stack = e.stack; }

var lazy = out;
