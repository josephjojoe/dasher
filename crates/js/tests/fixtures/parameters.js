// Parameters with default values and patterns, and the closures made in
// them: see the test that includes this file. Each case is a function
// whose result is recorded, or the name of what it threw.
(() => {
  const out = {};
  function t(name, f) {
    try {
      out[name] = f();
    } catch (e) {
      out[name] = 'threw ' + e.name;
    }
  }
  t('body assignment reaches the closure', () => { function f(a = 1, g = () => a) { a = 2; return g(); } return f(); });
  t('closure assignment reaches the body', () => { function f(a = 1, set = (v) => { a = v; }) { set(7); return a; } return f(); });
  t('compound assignment and update', () => { function f(a = 1, g = () => a) { a += 4; a++; return [a, g()]; } return f(); });
  t('a parameter without a default', () => { function f(a, g = () => a) { a = 'new'; return g(); } return f('old'); });
  t('an argument given for the default', () => { function f(a = 1, g = () => a) { a *= 3; return g(); } return f(5); });
  t('var in the body is a second variable', () => { function f(a = 1, g = () => a) { var a; a = 2; return [a, g()]; } return f(); });
  t('var in the body starts from the parameter', () => { function f(a = 1, g = () => a) { var a; return [a, g()]; } return f(9); });
  t('var with a value in the body', () => { function f(a = 1, g = () => a) { var a = 3; return [a, g()]; } return f(); });
  t('var in a loop of the body', () => { function f(i = 0, g = () => i) { for (var i = 0; i < 3; i++); return [i, g()]; } return f(); });
  t('a function declared in the body', () => { function f(a = 1, g = () => a) { function a() {} return [typeof a, g()]; } return f(); });
  t('a block-level let of the same name', () => { function f(a = 1, g = () => a) { { let a = 5; a++; } a = 2; return g(); } return f(); });
  t('a closure in the body', () => { function f(a = 1, g = () => a) { const bump = () => { a += 1; }; bump(); bump(); return [a, g()]; } return f(); });
  t('a function declaration in the body reads it', () => { function f(a = 1, set = (v) => { a = v; }) { function read() { return a; } set(4); return read(); } return f(); });
  t('closures outlive the call', () => { function f(a = 1, g = () => a) { return [() => ++a, g]; } const [inc, get] = f(); inc(); inc(); return get(); });
  t('each call has its own', () => { function f(a, g = () => a) { a += '!'; return g; } const x = f('x'), y = f('y'); return [x(), y()]; });
  t('recursion', () => { function f(n, g = () => n) { if (n > 0) f(n - 1); n *= 10; return g(); } return f(3); });
  t('object pattern', () => { function f({ a }, g = () => a) { a += 1; return [a, g()]; } return f({ a: 1 }); });
  t('array pattern with defaults', () => { function f([a = 1, b = 2] = [], g = () => [a, b]) { a = 'x'; b = 'y'; return g(); } return f(); });
  t('nested pattern', () => { function f({ p: { q } }, set = (v) => { q = v; }) { set('set'); return q; } return f({ p: { q: 0 } }); });
  t('pattern name redeclared with var', () => { function f({ a }, g = () => a) { var a = 5; return [a, g()]; } return f({ a: 1 }); });
  t('rest parameter', () => { function f(a = 0, ...rest) { const g = () => rest; rest = [9]; return g(); } return f(1, 2, 3); });
  t('rest with a closure before it', () => { function f(g = () => rest, ...rest) { rest = rest.concat('z'); return g(); } return f(undefined, 1); });
  t('a later parameter read by an earlier closure', () => { function f(g = () => b, b = 2) { b += 1; return g(); } return f(); });
  t('reading a later parameter too early', () => { function f(a = b, b = 1) { return a; } return f(); });
  t('arguments is not mapped', () => { function f(a, b = 1) { a = 5; return [arguments[0], arguments.length]; } return f(1); });
  t('arguments assigned to', () => { function f(a, b = () => a) { arguments[0] = 9; return [a, b()]; } return f(1); });
  t('arrow function', () => { const f = (a = 1, g = () => a) => { a = 2; return g(); }; return f(); });
  t('arrow function with an expression body', () => { const f = (a = 1, g = () => a) => (a = 2, g()); return f(); });
  t('method', () => { const o = { m(a = 1, g = () => a) { a = 2; return g(); } }; return o.m(); });
  t('constructor', () => { class C { constructor(a = 1, g = () => a) { a = 2; this.v = g(); } } return new C().v; });
  t('this in a default', () => { const o = { k: 3, m(a = this.k, g = () => a + this.k) { a = 10; return g(); } }; return o.m(); });
  t('generator', () => { function* f(a = 1, g = () => a) { a = yield g(); yield g(); a = 'end'; yield g(); } const it = f(); return [it.next().value, it.next('sent').value, it.next().value]; });
  t('strict function', () => { 'use strict'; function f(a = 1, g = () => a) { a = 2; return [g(), arguments[0]]; } return f(); });
  t('default refers to an earlier parameter', () => { function f(a = 1, b = a + 1, g = () => [a, b]) { a = 10; b = 20; return g(); } return f(); });
  t('default is a function declared later in the body', () => { function f(a = () => typeof h, g = () => a) { function h() {} return g()(); } return f(); });
  t('eval in the body', () => { function f(a = 1, g = () => a) { a = 2; return eval('a'); } return f(); });
  t('eval in a nested function', () => { function f(a = 1, g = () => a) { a = 2; return [(function () { return eval('a'); })(), g()]; } return f(); });
  t('eval in a nested arrow assigns', () => { function f(a = 1, g = () => a) { (() => eval('a = 5'))(); return [a, g()]; } return f(); });
  t('eval in a default', () => { function f(a = eval('1 + 1'), g = () => a) { a += 1; return g(); } return f(); });
  t('typeof and delete', () => { function f(a = 1, g = () => a) { return [typeof a, delete a, g()]; } return f(); });
  t('with in the body', () => { function f(a = 1, g = () => a) { with ({}) { a = 2; } return g(); } return f(); });
  t('with shadowing in the body', () => { function f(a = 1, g = () => a) { with ({ a: 0 }) { a = 2; } return [a, g()]; } return f(); });
  t('catch parameter of the same name', () => { function f(a = 1, g = () => a) { try { throw 0; } catch (a) { a = 5; } a = 2; return g(); } return f(); });
  t('label and loop closures', () => { function f(n = 3, g = () => n) { const fs = []; while (n > 0) { fs.push(() => n); n--; } return [fs.map((h) => h()), g()]; } return f(); });
  t('destructuring assignment in the body', () => { function f(a = 1, b = 2, g = () => [a, b]) { [a, b] = [b, a]; return g(); } return f(); });
  t('for-of over the parameter', () => { function f(a = 0, g = () => a) { for (a of [1, 2, 3]); return g(); } return f(); });
  t('many parameters', () => { function f(a = 1, b = 2, c = 3, d = 4, g = () => [a, b, c, d]) { d = a + b + c; a = b = c = 0; return g(); } return f(); });
  t('getter with no parameters and setter with a default', () => { const o = { set v(x = 1) { const g = () => x; x = 2; this.r = g(); } }; o.v = undefined; return o.r; });
  t('eval declares a var in a default', () => { function f(a = eval('var z = 4; 1'), g = () => [a, z]) { a = 2; return g(); } return f(); });
  t('class field initialiser arrow', () => { class C { m = (a = 1, g = () => a) => { a = 2; return g(); }; } return new C().m(); });
  t('default is a class', () => { function f(a = 1, K = class { static v() { return a; } }) { a = 2; return K.v(); } return f(); });
  t('immediately invoked in a default', () => { function f(a = 1, b = (() => a + 1)(), g = () => [a, b]) { a = 0; b = 0; return g(); } return f(); });
  t('big body', () => { function f(a = 1, g = () => a) { let s = 0; for (let i = 0; i < 100; i++) { s += i; if (i === 50) a = s; } function later() { return [a, g()]; } return later(); } return f(); });
  t('eval in the body, closure assigns', () => { function f(a = 1, set = (v) => { a = v; }) { set(5); return eval('a'); } return f(); });
  t('eval in a default and in the body', () => { function f(a = eval('1'), g = () => a) { a = 2; return [eval('a'), g()]; } return f(); });
  t('eval assigns in the body', () => { function f(a = 1, g = () => a) { eval('a = 7'); return [a, g()]; } return f(); });
  t('eval declares the name in the body', () => { function f(a = 1, g = () => a) { eval('var a = 5'); return [a, g()]; } return f(); });
  t('eval in the body with var in the body', () => { function f(a = 1, g = () => a) { var a = 3; return [eval('a'), g()]; } return f(); });
  t('eval in the body with a pattern', () => { function f({ a }, g = () => a) { a += 1; return [eval('a'), eval('a = 9'), g()]; } return f({ a: 1 }); });
  t('eval in the body makes a closure', () => { function f(a = 1, g = () => a) { const h = eval('() => ++a'); h(); h(); return [a, g()]; } return f(); });
  t('eval in a block of the body', () => { function f(a = 1, g = () => a) { { let b = 2; a = eval('a + b'); } return g(); } return f(); });
  t('strict eval in the body', () => { 'use strict'; function f(a = 1, g = () => a) { eval('a = 4; var a2 = 1'); return [a, g(), typeof a2]; } return f(); });
  t('eval in the body of a generator', () => { function* f(a = 1, g = () => a) { a = yield eval('a'); yield [eval('a'), g()]; } const it = f(); return [it.next().value, it.next(8).value]; });
  t('eval in the body of an arrow', () => { const f = (a = 1, g = () => a) => { a = 2; return [eval('a'), g()]; }; return f(); });
  t('eval reads arguments', () => { function f(a, b = () => a) { a = 3; return [eval('arguments[0]'), eval('a'), b()]; } return f(1); });
  t('nested eval in eval', () => { function f(a = 1, g = () => a) { eval('eval("a = 6")'); return [a, g()]; } return f(); });
  t('eval in the body, no closures in defaults', () => { function f(a = 1, b = a + 1) { a = 10; return [eval('a'), eval('b'), eval('a + b')]; } return f(); });
  t('eval declares the name, a closure in the body reads it', () => { function f(a = 1, g = () => a) { eval('var a = 5'); const h = () => a; return [a, h(), g()]; } return f(); });
  t('eval declares the name, then assignment', () => { function f(a = 1, g = () => a) { eval('var a = 5'); a = 6; return [a, eval('a'), g()]; } return f(); });
  t('eval declares the name of a pattern', () => { function f({ a }, g = () => a) { eval('var a = 5'); return [a, g()]; } return f({ a: 1 }); });
  t('eval declares another name', () => { function f(a = 1, g = () => a) { eval('var z = 5'); a = 2; return [a, z, g()]; } return f(); });
  t('eval declares a function of the name', () => { function f(a = 1, g = () => a) { eval('function a() {}'); return [typeof a, g()]; } return f(); });
  t('eval in a nested function declares the name', () => { function f(a = 1, g = () => a) { (function () { eval('var a = 5'); a = 6; })(); return [a, g()]; } return f(); });
  t('eval in the body and in a nested function', () => { function f(a = 1, g = () => a) { eval('a = 2'); return [(() => eval('a'))(), (function () { return eval('a = a + 1'); })(), a, g()]; } return f(); });
  t('eval declares the name, nested eval reads it', () => { function f(a = 1, g = () => a) { eval('var a = 5'); return [(() => eval('a'))(), g()]; } return f(); });
  t('eval let of the name', () => { function f(a = 1, g = () => a) { return [eval('let a = 5; a'), a, g()]; } return f(); });
  t('delete after eval declares the name', () => { function f(a = 1, g = () => a) { eval('var a = 5'); const d = delete a; return [d, a, g()]; } return f(); });
  t('a closure made before eval declares the name', () => { function f(a = 1, g = () => a) { const h = () => a; const before = h(); eval('var a = 5'); return [before, h(), g()]; } return f(); });
  t('a function in the body reads after eval declares the name', () => { function f(a = 1, g = () => a) { function h() { return a; } eval('var a = 5'); return [h(), g()]; } return f(); });
  t('a function two levels down reads after eval declares the name', () => { function f(a = 1, g = () => a) { function h() { return () => typeof a + a; } eval('var a = 5'); return [h()(), g()]; } return f(); });
  t('a function in the body assigns after eval declares the name', () => { function f(a = 1, g = () => a) { function h(v) { a = v; } eval('var a = 5'); h(8); return [a, g()]; } return f(); });
  t('a function in the body assigns, eval declares nothing', () => { function f(a = 1, g = () => a) { function h(v) { a = v; } eval('1'); h(8); return [a, g()]; } return f(); });
  t('a function in the body updates, eval declares the name later', () => { function f(a = 1, g = () => a) { function h() { return ++a; } const r = [h()]; eval('var a = 5'); r.push(h(), a, g()); return r; } return f(); });
  t('eval in a default declares the parameter', () => { function f(a = eval('var a = 3; 1'), g = () => a) { return [a, g()]; } return f(); });
  t('eval in a default declares a later parameter', () => { function f(a = eval('var b = 3; 1'), b = 2, g = () => [a, b]) { return g(); } return f(); });
  t('eval declares the name in a generator', () => { function* f(a = 1, g = () => a) { eval('var a = 5'); a = yield [a, g()]; yield [a, eval('a'), g()]; } const it = f(); return [it.next().value, it.next(9).value]; });
  t('eval declares the name in an arrow', () => { const f = (a = 1, g = () => a) => { eval('var a = 5'); return [a, g()]; }; return f(); });
  t('eval declares the name in a method', () => { const o = { m(a = 1, g = () => a) { eval('var a = 5'); a++; return [a, g()]; } }; return o.m(); });
  t('eval declares the name, with in the body', () => { function f(a = 1, g = () => a) { eval('var a = 5'); with ({ a: 7 }) { a = 8; } with ({}) { a += 1; } return [a, g()]; } return f(); });
  t('eval declares a rest parameter name', () => { function f(x = 1, ...a) { const g = () => a; eval('var a = 5'); return [a, g()]; } return f(0, 1, 2); });
  t('eval reads then declares in one call', () => { function f(a = 1, g = () => a) { return [eval('var b = a; var a = 5; [b, a]'), a, g()]; } return f(); });
  t('typeof and call through eval after it declares the name', () => { function f(a = () => 1, g = () => a()) { eval('var a = () => 2'); return [a(), eval('a()'), g()]; } return f(); });
  t('eval declares the name the closure assigns', () => { function f(a = 1, set = (v) => { a = v; }, g = () => a) { eval('var a = 5'); set(9); return [a, g()]; } return f(); });
  t('simple parameters, eval declares the name', () => { function f(a) { const g = () => a; eval('var a = 5'); return [a, g()]; } return f(1); });
  t('eval declares the name twice', () => { function f(a = 1, g = () => a) { eval('var a = 5'); eval('var a; a += 1'); return [a, g()]; } return f(); });
  return out;
})()
