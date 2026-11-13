// Object spread, object rest and Object.assign (tests/runtime.rs,
// property_copies_behave_the_same_on_every_path). An ordinary source is
// copied without a list of its keys, and an empty target can take the
// source's shape whole (see PATCHES.md). Getters, setters, symbols,
// index keys, non-enumerable properties, Proxies and sources changed in
// the middle of a copy take the general path or step down to it. The
// answers are the same with and without the change. V8 differs in two
// places where QuickJS does not follow the spec, before and after the
// change alike: a key deleted by a getter during the copy is still copied,
// as undefined, and spreading a string copies nothing.
(function () {
  const out = [];
  const p = (x) => out.push(x);
  const d = (o) => Reflect.ownKeys(o).map((k) => {
    const q = Object.getOwnPropertyDescriptor(o, k);
    const v = 'value' in q ? (typeof q.value === 'object' && q.value ? 'obj' : String(q.value)) : 'accessor';
    return String(k) + ':' + v + ':' + [q.writable, q.enumerable, q.configurable].map(Number).join('');
  }).join(' ');
  const log = [];
  const sym = Symbol('s');
  for (let i = 0; i < 3; i++) {
    const base = { a: 1, b: 2, c: 3 };
    p(d({ ...base }) + ' | ' + d({ ...base, d: i }) + ' | ' + d({ z: 0, ...base, a: 'over' }));
    p(d(Object.assign({}, base)) + ' | ' + d(Object.assign({}, base, { e: 1 }, null, undefined, 'xy')));
    const { a, ...rest } = base;
    p(a + ' ' + d(rest));
    const { [sym]: s1, ...rest2 } = { [sym]: 1, x: 2, [Symbol.for('g')]: 3 };
    p(String(s1) + ' ' + d(rest2));
    p(d({ ...{ 2: 'two', b: 1, 1: 'one', [sym]: 's', 4294967294: 'big', 4294967295: 'notindex' } }));
    const withHidden = Object.defineProperty({ v: 1 }, 'hidden', { value: 2, enumerable: false });
    Object.defineProperty(withHidden, 'ro', { value: 3, enumerable: true, writable: false });
    p(d({ ...withHidden }) + ' | ' + d(Object.assign({}, withHidden)));
    const getters = { get g() { log.push('get g'); return 'G'; }, plain: 1, get h() { log.push('get h'); delete this.plain2; this.added = 1; return 'H'; }, plain2: 2 };
    p(d({ ...getters }) + ' | ' + d(Object.assign({}, getters)));
    const changing = { first: 1, get second() { this.third = 'changed'; delete this.fourth; return 2; }, third: 3, fourth: 4 };
    p(d({ ...changing }));
    const target = { set a(v) { log.push('set a ' + v); }, b: 'keep' };
    Object.assign(target, base);
    p(d(target));
    Object.defineProperty(Object.prototype, 'trap', { set(v) { log.push('proto setter ' + v); }, configurable: true });
    p(d(Object.assign({}, { trap: 1, q: 2 })) + ' | ' + d({ ...{ trap: 1, q: 2 } }));
    delete Object.prototype.trap;
    Object.defineProperty(Object.prototype, 'ro2', { value: 0, writable: false, configurable: true });
    try { Object.assign({}, { ro2: 1 }); p('no error'); } catch (e) { p(e.constructor.name); }
    p(d({ ...{ ro2: 1 } }));
    delete Object.prototype.ro2;
    const frozen = Object.freeze({});
    try { Object.assign(frozen, { a: 1 }); p('no error'); } catch (e) { p(e.constructor.name); }
    const arr = Object.assign([1, 2], { 0: 'x', 5: 'y', length: 3 });
    p(JSON.stringify(arr) + ' ' + arr.length);
    const proxy = new Proxy({ a: 1, b: 2 }, { ownKeys(t) { log.push('ownKeys'); return Reflect.ownKeys(t); }, get(t, k) { log.push('get ' + String(k)); return t[k]; } });
    p(d({ ...proxy }) + ' | ' + d(Object.assign({}, proxy)));
    const proxyTarget = new Proxy({}, { set(t, k, v) { log.push('set ' + String(k)); t[k] = v; return true; }, defineProperty(t, k, desc) { log.push('define ' + String(k)); return Reflect.defineProperty(t, k, desc); } });
    Object.assign(proxyTarget, base);
    p(d(proxyTarget));
    const self = { a: 1, b: 2 };
    Object.assign(self, self);
    p(d(self));
    const protoed = Object.create({ inherited: 1 });
    protoed.own = 2;
    p(d({ ...protoed }) + ' | ' + d(Object.assign(Object.create(null), protoed)));
    const nullProtoSource = Object.assign(Object.create(null), { n: 1, m: 2 });
    p(d({ ...nullProtoSource }) + ' ' + Object.getPrototypeOf({ ...nullProtoSource }));
    const deleted = { a: 1, b: 2, c: 3 };
    delete deleted.b;
    p(d({ ...deleted }) + ' | ' + d(Object.assign({}, deleted)));
    p(d({ ...[1, 2] }) + ' | ' + d({ ...'hi' }) + ' | ' + d({ ...null, ...undefined, ...5 }));
    const fn = function () {}; fn.own = 1;
    p(d({ ...fn }));
    const { x, ...restOfMany } = { x: 1, y: 2, get z() { log.push('get z'); return 3; } };
    p(d(restOfMany));
    const dict = {};
    for (let k = 0; k < 40; k++) dict['k' + k] = k;
    p(Object.keys({ ...dict }).length + ' ' + Object.keys(Object.assign({}, dict, { k39: 'last' })).join().length);
  }
  p(log.join());
  return out;
})()
