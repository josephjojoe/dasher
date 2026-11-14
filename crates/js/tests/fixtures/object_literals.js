// What scripts can see of objects made by object literals and class
// fields (tests/runtime.rs, object_literals_have_the_properties_scripts_expect).
// A literal whose keys are all fixed names is made, after its first run,
// from the shape that run ended with, and define_field stores into a
// property that is already there (see PATCHES.md). Each literal here runs
// several times so that both paths are seen, and the answers are the same
// with and without the change and under V8.
(function () {
  const out = [];
  const d = (o) => Reflect.ownKeys(o).map((k) => {
    const p = Object.getOwnPropertyDescriptor(o, k);
    const v = 'value' in p ? (typeof p.value === 'object' && p.value ? 'obj' : typeof p.value === 'function' ? 'fn' : String(p.value)) : 'accessor';
    return String(k) + ':' + v + ':' + [p.writable, p.enumerable, p.configurable].map(Number).join('');
  }).join(' ');
  const p = (x) => out.push(x);
  let side = [];
  const f = (x) => { side.push(x); return x; };
  for (let i = 0; i < 3; i++) {
    const x = 'sx' + i;
    p(d({ a: i, b: 2, c: 'x', d: null }));
    p(d({ a: f(1), b: f(2), a: f(3) }));
    p(d({ 2: 'two', 1: 'one', z: 0, 'quoted key': 1, '0': 'zero', x }));
    p(d({ __proto__: null, a: 1 }) + ' proto:' + Object.getPrototypeOf({ __proto__: null, a: 1 }));
    p(d({ '__proto__': [], a: 1 }) + ' isArray:' + Array.isArray(Object.getPrototypeOf({ '__proto__': [], a: 1 })));
    const __proto__ = 5;
    p(d({ __proto__, a: 1 }));
    p(d({ [x]: 1, a: 2 }));
    p(d({ a: 1, ...{ b: 2, a: 3 }, c: 4 }));
    p(d({ m() { return 1; }, get g() { return 2; }, set g(v) {}, a: 1 }));
    p(d({ fn: function () {}, arrow: () => {}, cls: class {} }) + ' ' + [{ fn: function () {} }.fn.name, { arrow: () => {} }.arrow.name, { cls: class {} }.cls.name].join());
    p(d({ nested: { deep: { x: i } }, after: 1 }));
    const o = { a: 1, b: 2 };
    o.c = 3; delete o.a;
    p(d(o) + ' / ' + d({ a: 1, b: 2 }));
    const frozen = Object.freeze({ a: 1, b: 2 });
    p(d(frozen) + ' / ' + d({ a: 1, b: 2 }) + ' ' + Object.isFrozen({ a: 1, b: 2 }));
    const sealed = { k: 1 };
    Object.defineProperty(sealed, 'k', { enumerable: false });
    p(d(sealed) + ' / ' + d({ k: 1 }));
    try { p(d({ a: 1, b: (() => { throw new Error('in value'); })(), c: 3 })); } catch (e) { p(e.message); }
    let keys = [];
    for (const k in { b: 1, a: 2, 10: 3 }) keys.push(k);
    p(keys.join() + ' ' + JSON.stringify({ b: 1, a: [1, { c: 2 }], u: undefined }));
    const big = { k0: 0, k1: 1, k2: 2, k3: 3, k4: 4, k5: 5, k6: 6, k7: 7, k8: 8, k9: 9, k10: 10, k11: 11, k12: 12, k13: 13, k14: 14, k15: 15, k16: 16, k17: 17, k18: 18, k19: 19 };
    big.extra = 1;
    p(Object.keys(big).length + ' ' + Object.values(big).reduce((a, b) => a + b, 0));
    function* gen() { const r = { a: yield 1, b: yield 2 }; return r; }
    const g = gen(); g.next(); g.next('A'); p(d(g.next('B').value));
    const later = { a: 1 }; later.a = 'changed'; later.b = 2;
    p(d(later) + ' / ' + d({ a: 1 }));
  }
  // Setters and getters on Object.prototype are not called by a literal.
  Object.defineProperty(Object.prototype, 'trap', { set(v) { side.push('setter ' + v); }, get() { return 'proto'; }, configurable: true });
  for (let i = 0; i < 2; i++) p(d({ trap: i, other: 1 }) + ' ' + ({ other: 1 }).trap);
  delete Object.prototype.trap;
  // Class fields are defined with define_field on `this`.
  class A { a = 1; b = 2; }
  class B extends A { a = 3; c = 4; }
  class C { constructor() { Object.defineProperty(this, 'x', { value: 1, writable: true, enumerable: false, configurable: true }); } }
  class D extends C { x = 2; }
  class E { constructor() { Object.defineProperty(this, 'x', { value: 1, configurable: false }); } }
  class F extends E { x = 2; }
  class G { constructor() { Object.preventExtensions(this); } }
  class H extends G { y = 1; }
  for (let i = 0; i < 3; i++) {
    p(d(new A()) + ' / ' + d(new B()) + ' / ' + d(new D()));
    try { new F(); p('no error'); } catch (e) { p(e.constructor.name); }
    try { new H(); p('no error'); } catch (e) { p(e.constructor.name); }
    const prox = new Proxy({}, { defineProperty(t, k, desc) { side.push('trap ' + String(k)); return Reflect.defineProperty(t, k, desc); } });
    class Base { constructor() { return prox; } }
    class P extends Base { f = i; }
    new P();
  }
  p(side.join());
  return out;
})()
