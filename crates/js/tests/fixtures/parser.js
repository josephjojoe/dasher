// What the parser's fast paths must
// leave as it was: which bracket is an arrow function's parameters or a
// destructuring pattern when the lookahead stops early, the values of
// numbers, strings and names read the short way, and the order of binary
// operators. Each answer must be the one the engine gives without them.
var parser = (function () {
  var r = {};
  function syntax(src) {
    try { new Function(src); return 'ok'; } catch (e) { return e.name; }
  }

  // Destructuring assignments whose elements are not plain names.
  var a, b, c, d, o = { p: 0, q: [0, 0] }, rest;
  [o.p, o.q[1], (a), ...rest] = [1, 2, 3, 4, 5];
  r.targets = [o.p, o.q, a, rest];
  ({ x: o.p, y: [b, c] = [6, 7], 'z': d = 8, ...rest } = { x: 5, w: 9 });
  r.object = [o.p, b, c, d, rest];
  // Targets that look like literals, calls or functions, but are not.
  var lit = {};
  [{ k: 1 }.k, [2][0], function () {}.f, ''.length2] = [10, 20, 30, 40];
  r.oddTargets = [lit.k, typeof Function.prototype.f];
  [lit[`k${1}`], lit[(0, 'm')]] = ['t', 'p'];
  r.computed = [lit.k1, lit.m];
  // Literals that are not patterns, with and without '=' after them.
  r.literals = [[1, 'a', `b`, /c/.source, null, true, f(), x => x][0], { a: 1, b: 'two', c() { return 3; } }.c(),
                { get g() { return 4; } }.g, [{ a: 1 }, { a: 2 }][1].a, { k: [1, 2] }.k[1]];
  function f() { return 'call'; }
  r.notPatterns = [syntax('[1] = x'), syntax('[f()] = x'), syntax('({ m() {} } = x)'),
                   syntax('[a + b] = x'), syntax('[x => x] = y'), syntax('({ a: 1 } = x)'),
                   syntax('[[1]] = x'), syntax('[{ a: f() }] = x'), syntax('[a.b, `t`] = x'),
                   syntax('[{ a: 1 }.a] = x'), syntax('[(a)] = x'), syntax('[a?.b] = x')];

  // Arrow functions against bracketed expressions.
  var p = 1, q = 2;
  r.parens = [(p, q), (p), (p + q) * 2, ((p)), (0, f)(), (function () { return 7; })()];
  r.arrows = [((p, q) => p * q)(3, 4), ((p, { q } = { q: 5 }, ...more) => p + q + more.length)(1, undefined, 0, 0),
              ((p = (q) => q * 2) => p(3))(), (async (p) => p) instanceof Function,
              ((p) /* comment */ => p + 1)(1), ((p)	=> p + 2)(1)];
  r.notArrows = [syntax('(a.b) => 1'), syntax('(1) => 1'), syntax('(a)\n=> 1'), syntax('(a, f()) => 1'),
                 syntax('(a = 1, [b] = [], {c}) => 1'), syntax('async (a, b) => a')];
  // The header of a for loop.
  var loops = [];
  for (var i = 0, j = [1, 2].length; i < j; i++) loops.push(i);
  for (var k in { a: 1 }) loops.push(k);
  for (var [m, n] of [[3, 4]]) loops.push(m + n);
  for (let { length } of ['abc']) loops.push(length);
  for (;;) { loops.push('once'); break; }
  r.loops = loops;

  // Numbers, as js_atof() reads them.
  r.numbers = [0, 7, 123456789, 1234567890, 4294967296, 0.5, 5., .5, 1e3, 1_000, 0x1f, 0o17, 0b101,
               5..toString(), 5 .toFixed(1), 2147483647, 2147483648, -0 === 0, 1 / -0];
  r.numberTypes = [typeof 1n, typeof 10, 9007199254740993 === 9007199254740992];
  r.legacy = (function () { return [eval('017'), eval('019'), eval('08')]; })();
  r.badNumbers = [syntax('3in x'), syntax('1_'), syntax('0_1'), syntax('"use strict"; 017'), syntax('1.e'), syntax('5a')];

  // Strings, plain and with escapes.
  r.strings = ['', 'plain', "double", 'it\'s', 'tab\there', 'café', 'café', '\x41', 'line\
continued', 'a"b', "a'b", '1234', '\0'];
  r.stringLengths = r.strings.map(function (s) { return s.length; });
  r.badStrings = [syntax("'unterminated"), syntax("'line\nbreak'"), syntax("'\\u{110000}'"), syntax('"use strict"; "\\01"')];

  // Names, plain and not.
  var abc = 'escaped', ünïcode = 'unicode', $_ = 'symbols', of = 'of', yield_ = 'y';
  r.names = [abc, ünïcode, $_, of, yield_];
  r.badNames = [syntax('var \\u0069f = 1'), syntax('var if = 1'), syntax('"use strict"; var let = 1')];

  // Binary operators, every level against its neighbours.
  r.binary = [1 + 2 * 3 - 4 / 2 % 3, 1 << 2 + 1, 64 >> 1 >>> 1 << 1, 3 < 4 == true, 1 & 3 | 4 ^ 6,
              2 ** 3 ** 2, (-2) ** 2, 1 < 2 < 3, 3 > 2 > 1, 'a' in { a: 1 } === true,
              [] instanceof Array & 1, 1 | 2 & 3 ^ 4 == 4, 10 - 2 - 3, 2 * 3 % 4, 1 + - + 1];
  var inLoop = [];
  for (var t = ('x' in { x: 1 }) ? 'in' : 'out'; inLoop.length < 1;) inLoop.push(t);
  r.inOperator = inLoop;
  class Priv { #x = 1; static has(o) { return #x in o === true && !(#x in {}); } }
  r.privateIn = [Priv.has(new Priv()), syntax('class C { #x; m(o) { return #x in o < 1; } }')];
  return JSON.stringify(r);
})();
