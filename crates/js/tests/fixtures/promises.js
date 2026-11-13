// Promises, async functions and the functions the engine makes in C
// (tests/runtime.rs, promises_and_functions_behave_the_same_on_every_path).
// A capability for the realm's own Promise is made without its executor,
// functions made in C take `length` and `name` from a remembered shape,
// and naming a new function stores its `name` in place (see PATCHES.md).
// The script records the order in which reactions run and what can be
// seen of each function into `promiseLog`, which is complete once all
// jobs have run. The answers are the same with and without the change
// and under V8.
var promiseLog = [];
(function () {
  'use strict';
  const log = (x) => promiseLog.push(String(x));
  const d = (f) => Object.getOwnPropertyNames(f).map((k) => {
    const q = Object.getOwnPropertyDescriptor(f, k);
    return k + ':' + (typeof q.value === 'object' && q.value ? 'obj' : String(q.value)) + ':' + [q.writable, q.enumerable, q.configurable].map(Number).join('');
  }).join(' ');
  let res, rej;
  const p0 = new Promise((a, b) => { res = a; rej = b; });
  log('resolving functions ' + d(res) + ' | ' + d(rej) + ' ' + (res !== rej));
  log('builtins ' + d(Math.max) + ' | ' + d(Promise.resolve) + ' | ' + d(Array.prototype.map) + ' | ' + d(function () {}.bind(null)));
  const arrow = () => {}; const fexpr = function () {}; const named = function inner() {};
  const holder = { m: () => {}, n: function () {} }; class K { f = () => {}; static g = () => {}; }
  const C = class {}; const D = class { static name() { return 'static'; } };
  let late; late = () => {};
  const [destr = () => {}] = [];
  const renamed = () => {}; Object.defineProperty(renamed, 'name', { value: 'fixed' });
  log('names ' + [arrow.name, fexpr.name, named.name, holder.m.name, holder.n.name, new K().f.name, K.g.name, C.name, typeof D.name, late.name, destr.name, renamed.name].join());
  log('arrow ' + d(arrow) + ' | ' + d(fexpr));
  for (let i = 0; i < 3; i++) { const fresh = () => i; log(fresh.name + ' ' + d(fresh)); }
  p0.then((v) => log('p0 then ' + v));
  Promise.resolve(1).then((v) => log('a ' + v)).then(() => log('a2'));
  Promise.resolve(2).then((v) => { log('b ' + v); return Promise.resolve('inner'); }).then((v) => log('b2 ' + v));
  Promise.reject(new Error('r')).catch((e) => log('caught ' + e.message)).finally(() => log('finally'));
  const thenable = { then(f) { log('thenable then called'); f('from thenable'); } };
  Promise.resolve(thenable).then((v) => log('t ' + v));
  const pr = Promise.resolve(3);
  log('same ' + (Promise.resolve(pr) === pr));
  class MyPromise extends Promise { constructor(ex) { super(ex); log('MyPromise constructed'); } }
  const mp = MyPromise.resolve(4);
  log('subclass ' + (mp instanceof MyPromise) + ' ' + (mp.then(() => {}) instanceof MyPromise));
  const fakeCtor = Promise.resolve(5); fakeCtor.constructor = function (ex) { log('fake ctor'); return new Promise(ex); };
  fakeCtor.constructor[Symbol.species] = fakeCtor.constructor;
  fakeCtor.then((v) => log('fake ' + v));
  async function af(x) { log('af start ' + x); const y = await x; log('af after await ' + y); return y + 1; }
  async function thrower() { await null; throw new Error('async throw'); }
  async function awaitsThenable() { return await thenable; }
  async function awaitsRejected() { try { await Promise.reject('no'); } catch (e) { return 'recovered ' + e; } }
  async function returnsPromise() { return Promise.resolve('returned promise'); }
  af(10).then((v) => log('af result ' + v));
  af(Promise.resolve(20)).then((v) => log('af2 result ' + v));
  thrower().catch((e) => log('thrower ' + e.message));
  awaitsThenable().then((v) => log('awaitsThenable ' + v));
  awaitsRejected().then((v) => log(v));
  returnsPromise().then((v) => log(v));
  Promise.all([1, Promise.resolve(2), thenable]).then((v) => log('all ' + v.join()));
  Promise.allSettled([Promise.reject(1), 2]).then((v) => log('allSettled ' + v.map((r) => r.status).join()));
  Promise.race([new Promise(() => {}), Promise.resolve('race')]).then((v) => log(v));
  Promise.any([Promise.reject(1), Promise.resolve('any')]).then((v) => log(v));
  (async () => { let s = 0; for (let i = 0; i < 5; i++) s = await (async (x) => x + 1)(s); log('loop ' + s); })();
  let chain = Promise.resolve(0);
  for (let i = 0; i < 5; i++) chain = chain.then((x) => x + 1);
  chain.then((x) => log('chain ' + x));
  const withResolvers = Promise.withResolvers ? Promise.withResolvers() : null;
  if (withResolvers) { withResolvers.promise.then((v) => log('withResolvers ' + v)); withResolvers.resolve('ok'); }
  res('resolved late');
  log('sync end');
})();
