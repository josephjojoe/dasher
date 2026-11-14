// Postfix ++ and -- and assignment statements on `let` variables
// (tests/runtime.rs, increments_give_the_same_values_on_every_path): an
// int has a fast path in the interpreter and the compiler rewrites
// `i++;` and `x = e;` on a `let`; every other value, and the int limits,
// take the general path. Run with and without the change, and under V8,
// this gives the same answers.
(function () {
  const out = [];
  const s = (x) => typeof x === 'bigint' ? x + 'n' : Object.is(x, -0) ? '-0' : typeof x === 'object' && x ? 'obj' : String(x);
  const p = (...a) => out.push(a.map(s).join(' '));
  const values = [0, -1, 1, 2147483646, 2147483647, -2147483647, -2147483648, 1.5, -0, NaN,
    Infinity, '5', 'x', '', null, undefined, true, 10n, [], [3], {}, { valueOf() { return 41; } }];
  for (const v of values) {
    let a = v, b = v;
    const r1 = a++, r2 = b--;
    p(typeof r1, r1, a, typeof r2, r2, b);
    let c = v; c++; let d = v; d--;
    p(c, d);
    var e = v; const r3 = e++;
    p(r3, e);
  }
  let t = 0;
  for (let i = 2147483640; i < 2147483650; i++) t += i;
  p(t);
  for (let i = -2147483645; i > -2147483652; i--) t -= i;
  p(t);
  try { (() => { x++; let x = 1; })(); } catch (err) { p(err.constructor.name); }
  try { (() => { let y = (y++, 1); return y; })(); } catch (err) { p(err.constructor.name); }
  try { (() => { z = 2; let z; })(); } catch (err) { p(err.constructor.name); }
  let last;
  for (let k = 0; k < 3; k++) { last = k; }
  p(last);
  const fs = [];
  for (let k = 0; k < 3; k++) fs.push(() => k);
  p(fs.map((f) => f()).join());
  let q = { valueOf() { p('valueOf'); return 7; } };
  const old = q++;
  p(old, q);
  let w = 1; w = w + 1; p(w);
  let u = 0;
  for (let i = 0; i < 5; i++) u = u + i;
  p(u);
  return out;
})()
