// The RegExp methods on regexps a script has left alone and on regexps it
// has changed. dasher runs the methods straight on the matcher while
// nothing they would look at has been changed (js_regexp_pristine() in
// quickjs.c, see vendor/rquickjs-sys/PATCHES.md); tests/regexp.rs runs
// this file that way and the engine's own way and expects the same
// answers. Each case is a function whose result, or what it throws, goes
// into `out`.
var cases = {};
var log = [];
function note(x) { log.push(x); }
function str(v) {
  if (v === undefined) return 'undefined';
  try { return JSON.stringify(v); } catch (e) { return String(v); }
}
function all(re, s) {
  var r = [];
  re.lastIndex = 0;
  r.push(str(re.exec(s)), re.lastIndex);
  re.lastIndex = 0;
  r.push(re.test(s), re.lastIndex);
  r.push(s.replace(re, '[$&|$1|$`|$\'|$$|$2|$10|$01|$<n>|$]'), re.lastIndex);
  r.push(s.replace(re, function () { note(Array.prototype.slice.call(arguments)); return '#'; }), re.lastIndex);
  r.push(str(s.match(re)), re.lastIndex);
  r.push(str(s.split(re)), str(s.split(re, 2)), str(s.split(re, 0)), str(s.split(re, -1)), str(s.split(re, 2.5)), str(s.split(re, 4294967297)), re.lastIndex);
  re.lastIndex = 3;
  r.push(s.search(re), re.lastIndex);
  if (re.global) {
    var m = [];
    for (var x of s.matchAll(re)) m.push(x.index + ':' + x[0]);
    r.push(m.join(','));
  }
  return r;
}
var subjects = ['', 'a-b-c', 'foo boo zoo', 'item 123-abc 45-de', 'x😀y😀', 'line1\nline2', 'aaa'];
var patterns = [/o/g, /o/, /-/, /(-)/, /(\d+)-(\w+)/g, /(\d+)|(x)/g, /(?:)/g, /(?:)/gu, /x*/g, /\b/g, /$/gm, /o/y, /o/gy, /😀/gu, /./gu, /(?<n>o)/g, /(?<n>\d+)-(?<w>\w+)/, /(a)|b/g, /a/d, /(a)(?<n>b)?/dg];
patterns.forEach(function (re) {
  subjects.forEach(function (s) {
    cases['plain ' + re + ' ' + JSON.stringify(s)] = function () { return all(re, s); };
  });
});

// lastIndex that is not a small integer, or that runs a script when read
cases['lastIndex float'] = function () { var re = /o/g; re.lastIndex = 1.5; return [re.test('foo'), re.lastIndex, (re.lastIndex = 2.5, re.exec('foo').index), re.lastIndex]; };
cases['lastIndex string'] = function () { var re = /o/y; re.lastIndex = '1'; return [re.test('foo'), re.lastIndex]; };
cases['lastIndex negative'] = function () { var re = /o/g; re.lastIndex = -5; return [re.test('foo'), re.lastIndex, 'foo'.replace(re, 'x'), re.lastIndex]; };
cases['lastIndex huge'] = function () { var re = /o/g; re.lastIndex = 2147483648; return [re.test('foo'), re.lastIndex]; };
cases['lastIndex -0 search'] = function () { var re = /o/g; re.lastIndex = -0; var r = 'foo'.search(re); return [r, Object.is(re.lastIndex, -0)]; };
cases['lastIndex object'] = function () {
  var re = /(o)/g; var n = 0;
  re.lastIndex = { valueOf: function () { n++; return 1; } };
  return [re.test('foo'), n, re.lastIndex, str(re.exec('foo')), n];
};
cases['lastIndex recompiles'] = function () {
  var re = /(a)(b)?/g;
  re.lastIndex = { valueOf: function () { re.compile('(x)(y)(z)(w)(v)(u)(t)(s)(r)(q)(p)(o)(n)(m)(l)(k)(j)(i)(h)(g)', 'g'); return 0; } };
  return str(re.exec('xyzwvutsrqponmlkjihg ab'));
};
cases['lastIndex not writable g'] = function () {
  var re = /o/g; Object.defineProperty(re, 'lastIndex', { writable: false });
  var r = [];
  try { r.push(re.test('foo')); } catch (e) { r.push(e.name); }
  try { r.push('foo'.replace(re, 'x')); } catch (e) { r.push(e.name); }
  try { r.push(str('foo'.match(re))); } catch (e) { r.push(e.name); }
  try { r.push(str('foo'.split(re))); } catch (e) { r.push(e.name); }
  try { r.push('foo'.search(re)); } catch (e) { r.push(e.name); }
  return r;
};
cases['lastIndex not writable non-global'] = function () {
  var re = /o/; Object.defineProperty(re, 'lastIndex', { writable: false, value: 0 });
  return [re.test('foo'), 'foo'.replace(re, 'x'), str('foo'.match(re)), str('foo'.split(re)), 'foo'.search(re)];
};
cases['frozen'] = function () {
  var re = Object.freeze(/o/); var g = Object.freeze(/o/g); var r = [re.test('foo'), 'foo'.replace(re, 'x')];
  try { r.push(g.test('foo')); } catch (e) { r.push(e.name); }
  return r;
};

// a changed RegExp or RegExp.prototype
function patched(fn) {
  var saved = {};
  var names = Object.getOwnPropertyNames(RegExp.prototype).concat(Object.getOwnPropertySymbols(RegExp.prototype));
  names.forEach(function (k) { saved[typeof k === 'symbol' ? k.toString() : k] = [k, Object.getOwnPropertyDescriptor(RegExp.prototype, k)]; });
  var species = Object.getOwnPropertyDescriptor(RegExp, Symbol.species);
  try { return fn(); } finally {
    Object.keys(saved).forEach(function (k) { Object.defineProperty(RegExp.prototype, saved[k][0], saved[k][1]); });
    Object.defineProperty(RegExp, Symbol.species, species);
  }
}
function watched() { return ['foo boo', 'a-b-c'].map(function (s) { return [all(/o/g, s), all(/(-)/, s), all(/o/y, s)]; }); }
cases['own exec'] = function () {
  var re = /o/g; re.exec = function (s) { note('own exec ' + s + ' ' + this.lastIndex); return RegExp.prototype.exec.call(this, s); };
  return all(re, 'foo boo');
};
cases['prototype exec'] = function () {
  return patched(function () {
    var exec = RegExp.prototype.exec;
    RegExp.prototype.exec = function (s) { note('exec ' + this.source + ' ' + this.lastIndex); return exec.call(this, s); };
    return watched();
  });
};
cases['prototype exec returns junk'] = function () {
  return patched(function () {
    RegExp.prototype.exec = function () { return { 0: 'zz', index: 1, length: 1, groups: undefined }; };
    var r = [];
    try { r.push(/o/.test('foo'), 'foo'.replace(/o/, 'x'), str('foo'.match(/o/)), 'foo'.search(/o/)); } catch (e) { r.push(e.name); }
    return r;
  });
};
// A getter that notes each call and gives the answer the engine's would.
function spy(name, answer) {
  var get = Object.getOwnPropertyDescriptor(RegExp.prototype, name).get;
  Object.defineProperty(RegExp.prototype, name, {
    get: function () { note(name + ' ' + this.source); return answer ? answer.call(this) : get.call(this); },
    configurable: true,
  });
}
['flags', 'global', 'unicode', 'sticky', 'hasIndices', 'ignoreCase'].forEach(function (name) {
  cases[name + ' getter'] = function () {
    return patched(function () { spy(name); return watched(); });
  };
});
cases['unicode getter says yes'] = function () {
  return patched(function () {
    spy('unicode', function () { return true; });
    return ['x😀y'.replace(/(?:)/g, '-'), str('x😀y'.match(/(?:)/g))];
  });
};
cases['flags getter adds y'] = function () {
  return patched(function () {
    var flags = Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get;
    spy('flags', function () { return flags.call(this) + (this.sticky ? '' : 'y'); });
    return [str('a-b-c'.split(/-/)), str('a-b-c'.split(/b/))];
  });
};
cases['deleted getter'] = function () {
  return patched(function () {
    delete RegExp.prototype.hasIndices;
    return watched();
  });
};
cases['added property'] = function () {
  return patched(function () {
    RegExp.prototype.extra = 1;
    var r = watched();
    delete RegExp.prototype.extra;
    return r;
  });
};
cases['constructor'] = function () {
  return patched(function () {
    function Fake(src, flags) { note('Fake ' + flags); return new RegExp(src, flags); }
    RegExp.prototype.constructor = Fake;
    return watched();
  });
};
cases['species'] = function () {
  return patched(function () {
    Object.defineProperty(RegExp, Symbol.species, { get: function () { note('species'); return function (src, flags) { note('made ' + flags); return new RegExp(src, flags); }; }, configurable: true });
    return watched();
  });
};
cases['own constructor'] = function () {
  var re = /-/; re.constructor = function (src, flags) { note('own ctor ' + flags); return new RegExp(src, flags); };
  return [str('a-b'.split(re)), all(re, 'a-b')];
};
cases['subclass'] = function () {
  class R extends RegExp { exec(s) { note('R exec ' + this.lastIndex); return super.exec(s); } }
  var re = new R('o', 'g');
  return [all(re, 'foo boo'), str('a-b'.split(new R('-')))];
};
cases['proto swapped'] = function () {
  var re = /o/g; Object.setPrototypeOf(re, Object.create(RegExp.prototype));
  return all(re, 'foo');
};
cases['replace callback changes things'] = function () {
  var re = /o/g;
  var r = 'foo boo'.replace(re, function (m, i) {
    note('cb ' + i + ' ' + re.lastIndex);
    re.lastIndex = 5;
    RegExp.prototype.exec = RegExp.prototype.exec;
    return String(re.lastIndex);
  });
  return [r, re.lastIndex];
};
cases['replace callback this and arity'] = function () {
  return 'a1b22'.replace(/(\d)(\d)?/g, function () { 'use strict'; return [typeof this, arguments.length].join(); });
};
cases['replace callback throws'] = function () {
  try { return 'abc'.replace(/b/g, function () { throw new Error('boom'); }); } catch (e) { return e.message; }
};
cases['replace value toString changes exec'] = function () {
  return patched(function () {
    var re = /o/g;
    var rep = { toString: function () { RegExp.prototype.exec = function () { note('late exec'); return null; }; return 'x'; } };
    return ['foo'.replace(re, rep), re.lastIndex];
  });
};
cases['test argument toString changes exec'] = function () {
  return patched(function () {
    var arg = { toString: function () { RegExp.prototype.exec = function () { note('late exec'); return null; }; return 'foo'; } };
    return [/o/.test(arg), /o/.test('foo')];
  });
};
cases['split limit valueOf changes exec'] = function () {
  return patched(function () {
    var lim = { valueOf: function () { RegExp.prototype.exec = function () { note('late exec ' + this.lastIndex); return null; }; return 5; } };
    return str('a-b-c'.split(/-/, lim));
  });
};
cases['dollar patterns'] = function () {
  var s = 'abc-def-ghi';
  return ['$', '$$', '$&$&', '$`', "$'", '$0', '$1', '$2', '$3', '$9', '$10', '$11', '$01', '$001', '$<a>', '$<', 'x$', '$$1', '$1$', '$&-$1-$2'].map(function (rep) {
    return [s.replace(/(\w)(\w)-/g, rep), s.replace(/-/, rep), s.replace(/(?<a>\w)-/g, rep), s.replace(/(a)|(-)/g, rep)];
  });
};
cases['named groups'] = function () {
  var re = /(?<y>\d{4})-(?<m>\d\d)(?:-(?<d>\d\d))?/g;
  var s = '2026-11 and 2027-01-02';
  return [str(re.exec(s)), str(re.exec(s)), s.replace(re, '$<d>/$<m>/$<y>'), s.replace(re, function () { return str(arguments[arguments.length - 1]); }), str(s.split(/(?<x>-)/))];
};
cases['exec result shape'] = function () {
  var r = [];
  [/(a)(b)?/, /(a)(b)?/d, /(?<n>a)(b)?/, /a/, /(?<n>a)/d].forEach(function (re) {
    var m = re.exec('xab');
    var d = Object.getOwnPropertyNames(m).map(function (k) { var p = Object.getOwnPropertyDescriptor(m, k); return k + ':' + p.writable + p.enumerable + p.configurable; });
    r.push(Object.keys(m).join(), d.join(), Array.isArray(m), m.length, str(m.groups), str(m.indices), Object.getPrototypeOf(m) === Array.prototype);
    m.push('z'); m.extra = 1; r.push(str(m), m.length);
    var m2 = re.exec('xab'); r.push(Object.keys(m2).join(), m2.extra);
  });
  return r;
};
cases['same source many regexps'] = function () {
  var a = new RegExp('o', 'g'), b = new RegExp('o', 'g'), c = new RegExp('o', 'y'), d = new RegExp('o');
  a.lastIndex = 2;
  var r = [a.test('foo'), a.lastIndex, b.test('foo'), b.lastIndex, c.test('foo'), d.test('foo'), a === b, a.source === b.source];
  b.compile('x', 'g');
  r.push(a.test('xo'), a.lastIndex, b.test('xo'), b.lastIndex, new RegExp('o', 'g').exec('xo').index);
  for (var i = 0; i < 100; i++) r.push(new RegExp('(^|; )k' + (i % 7) + '=([^;]*)').exec('a=1; k' + (i % 5) + '=v' + i) + '');
  try { new RegExp('(', 'g'); } catch (e) { r.push(e.name); }
  try { new RegExp('(', 'g'); } catch (e) { r.push(e.name); }
  r.push(new RegExp('a', 'gi').flags, new RegExp('a', 'ig').flags, new RegExp('a', 'u').unicode, new RegExp('a').unicode);
  return r;
};
cases['split'] = function () {
  var r = [];
  ['', 'a', 'a,b', ',a,', 'a,,b', 'x😀y', '😀'].forEach(function (s) {
    [/,/, /(,)/, /,/y, /(?:)/, /(?:)/u, /(?:)/y, /x*/, /$/, /^/m, /\b/, /(,)|(;)/, /\ude00/, /\ude00/u, /(?=,)/].forEach(function (re) {
      r.push(str(s.split(re)), str(s.split(re, 1)), str(s.split(re, 3)), re.lastIndex);
    });
  });
  return r;
};
cases['match and matchAll'] = function () {
  var r = [];
  ['', 'aXbX', 'x😀y', 'aaa'].forEach(function (s) {
    [/X/g, /(?:)/g, /(?:)/gu, /a*/g, /a/gy, /./gu, /X/].forEach(function (re) {
      re.lastIndex = 1;
      r.push(str(s.match(re)), re.lastIndex);
      var m = []; if (re.global) { for (var x of s.matchAll(re)) m.push(x.index); }
      r.push(m.join(), re.lastIndex);
    });
  });
  return r;
};
cases['search'] = function () {
  var r = [];
  [/o/, /o/g, /o/y, /x/g, /$/].forEach(function (re) {
    re.lastIndex = 2; r.push('foo'.search(re), re.lastIndex);
    re.lastIndex = 'q'; r.push('foo'.search(re), re.lastIndex);
  });
  return r;
};
cases['non-string subjects'] = function () {
  return [/1/.test(123), 'a1b'.replace(/\d/, 2), str(/(\d)/.exec({ toString: function () { return 'x9'; } })), str(String.prototype.split.call(12321, /2/)), /null/.test(null), /undefined/.test()];
};
cases['long subject'] = function () {
  var s = 'word '.repeat(20000) + 'x@y.zz ' + 'tail '.repeat(1000);
  var re = /[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+/g;
  return [str(s.match(re)), s.replace(/\s+/g, ' ').length, s.split(/o/).length, (s + '’').search(/@/), /\b(?:tail|nope)\b/.exec(s).index, /[^w]+$/.exec(s).index];
};

var results = {};
Object.keys(cases).forEach(function (k) {
  log = [];
  var r;
  try { r = cases[k](); } catch (e) { r = 'THROW ' + e.name + ': ' + e.message; }
  results[k] = [r, log];
});
var out = JSON.stringify(results);
