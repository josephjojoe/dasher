// for-in, Object.keys, Object.values, Object.entries and
// Object.getOwnPropertyNames (tests/runtime.rs,
// enumeration_gives_the_same_keys_on_every_path). An ordinary object of
// plain string-keyed data properties lends its shape to the for-in
// iterator, and its keys and values go straight into an array; ordinary
// prototypes are checked for enumerable keys from their shapes.
// Objects changed during the loop, index keys, symbols,
// accessors, enumerable keys on prototypes, Proxies and exotic objects
// take the general path. The answers are the same with and without the
// change and under V8.
(function () {
  'use strict';
  const out = [];
  const p = (x) => out.push(x);
  const forIn = (o) => { const ks = []; for (const k in o) ks.push(k); return ks.join(); };
  const all = (o) => [forIn(o), Object.keys(o).join(), JSON.stringify(Object.values(o)), JSON.stringify(Object.entries(o)), Object.getOwnPropertyNames(o).join()].join(' | ');
  const log = [];
  for (let round = 0; round < 3; round++) {
    const o = { a: 1, b: 2, c: 3, d: 4, e: 5 };
    p(all(o));
    const del = { a: 1, b: 2, c: 3 };
    delete del.b;
    p(all(del));
    const hidden = Object.defineProperty({ x: 1, y: 2 }, 'h', { value: 3, enumerable: false });
    p(all(hidden));
    p(all({ 2: 'b', 1: 'a', z: 1, '10': 'c', '4294967295': 'n' }));
    const sym = { s: 1, [Symbol('k')]: 2, t: 3 };
    p(all(sym));
    const acc = { get g() { log.push('getter'); return 'G'; }, v: 1 };
    p(all(acc));
    p(all(Object.assign(Object.create(null), { n: 1, m: 2 })));
    p(all(Object.create({ inherited: 1, shadowed: 2 }, { own: { value: 3, enumerable: true }, shadowed: { value: 4, enumerable: false } })));
    p(all([1, 2, 3]) + ' # ' + all('str') + ' # ' + all(function f() {}));
    class K { constructor() { this.i = 1; this.j = 2; } m() {} }
    p(all(new K()));
    p(all(Object.freeze({ f: 1, g: 2 })));
    p(all(new Proxy({ px: 1, py: 2 }, { ownKeys(t) { log.push('ownKeys'); return Reflect.ownKeys(t); } })));
    const mutating = { a: 1, b: 2, c: 3, d: 4 };
    const seen = [];
    for (const k in mutating) { seen.push(k); if (k === 'a') { delete mutating.c; mutating.added = 1; } }
    p('mutated during for-in: ' + seen.join());
    const mutating2 = { a: 1, b: 2 };
    const seen2 = [];
    for (const k in mutating2) { seen2.push(k); if (k === 'a') { delete mutating2.b; mutating2.b = 'back'; } }
    p('deleted and re-added: ' + seen2.join());
    const seen3 = [];
    const reshaped = { a: 1, b: 2, c: 3 };
    for (const k in reshaped) { seen3.push(k); Object.defineProperty(reshaped, 'b', { enumerable: false }); }
    p('made non-enumerable: ' + seen3.join());
    // An object whose shape is its own (after a delete) gains and loses
    // keys during the loop.
    const own = { a: 1, b: 2, c: 3, d: 4 };
    delete own.b;
    const seen4 = [];
    for (const k in own) { seen4.push(k); own['n' + k] = 1; delete own.d; }
    p('own shape changed during for-in: ' + seen4.join() + ' / ' + Object.keys(own).join());
    const nested = { x: { y: 1 }, z: 2 };
    const pairs = [];
    for (const k in nested) for (const k2 in nested) pairs.push(k + k2);
    p(pairs.join());
    const big = {};
    for (let i = 0; i < 30; i++) big['p' + i] = i;
    p(Object.keys(big).length + ' ' + forIn(big).length + ' ' + Object.values(big).reduce((a, b) => a + b, 0));
    if (round === 1) {
      Object.prototype.enumerableOnProto = 'yes';
      p('with enumerable on Object.prototype: ' + all({ q: 1 }) + ' # ' + forIn([1]));
      delete Object.prototype.enumerableOnProto;
      Object.defineProperty(Object.prototype, 'hiddenOnProto', { value: 1, enumerable: false, configurable: true });
      p('with hidden on Object.prototype: ' + all({ q: 1, hiddenOnProto: 2 }));
      delete Object.prototype.hiddenOnProto;
    }
  }
  p(log.join());
  return out;
})()
