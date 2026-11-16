// `var` in code a direct eval runs inside a function: see the test that
// includes this file. Each case is a function whose result is recorded,
// or the name of what it threw.
(() => {
  const out = {};
  function t(name, f) {
    try {
      out[name] = f();
    } catch (e) {
      out[name] = 'threw ' + e.name;
    }
  }
  t('a second eval declares the variable again', () => { function f() { eval('var a = 5'); eval('var a; a += 1'); return a; } return f(); });
  t('eval declares a parameter twice', () => { function f(a) { eval('var a = 5'); eval('var a; a += 1'); return a; } return f(1); });
  t('a second eval assigns', () => { function f() { eval('var a = 5'); eval('a += 1'); return a; } return f(); });
  t('the variable keeps its value', () => { function f() { eval('var a = 5'); return eval('var a; typeof a'); } return f(); });
  t('with default values, the name of a parameter', () => { function f(a = 1, g = () => a) { eval('var a = 5'); return eval('var a; typeof a'); } return f(); });
  t('with default values, another name', () => { function f(a = 1, g = () => a) { eval('var z = 5'); return eval('var z; typeof z'); } return f(); });
  t('with a default value and no closure', () => { function f(b = 1) { eval('var z = 5'); return eval('var z; typeof z'); } return f(); });
  t('with default values, a second eval assigns', () => { function f(a = 1, g = () => a) { eval('var a = 5'); return eval('a += 1; a'); } return f(); });
  t('a function, then var', () => { function f() { eval('function a() {}'); return eval('var a; typeof a'); } return f(); });
  t('var, then a function', () => { function f() { eval('var a = 5'); return eval('function a() {} typeof a'); } return f(); });
  t('var with a value twice', () => { function f() { eval('var a = 5'); eval('var a = 7'); return a; } return f(); });
  t('twice in one eval', () => { function f() { return eval('var a = 5; var a; a'); } return f(); });
  t('the first declaration is undefined', () => { function f() { return eval('var a; [typeof a, "a" in {}]'); } return f(); });
  t('in a loop', () => { function f() { let s = 0; for (let i = 0; i < 3; i++) eval('var n; n = (n || 0) + 1; s = n'); return s; } return f(); });
  t('declared in a default value, then in the body', () => { function f(x = eval('var z = 4; 1')) { return [eval('var z; typeof z'), z]; } return f(); });
  t('a nested function has its own', () => { function f() { eval('var a = 1'); (() => { eval('var a = 2'); eval('var a; a += 1'); })(); return a; } return f(); });
  t('strict eval keeps its own', () => { 'use strict'; function f() { eval('var a = 5'); return eval('var a; typeof a'); } return f(); });
  t('indirect eval, on the global object', () => { (0, eval)('var __tw = 5'); return (0, eval)('var __tw; typeof __tw'); });
  t('a name of Object.prototype', () => { function f() { return eval('var toString; typeof toString'); } return f(); });
  t('deleted, then declared again', () => { function f() { eval('var a = 5'); eval('delete a'); return eval('var a; typeof a'); } return f(); });
  return out;
})()
