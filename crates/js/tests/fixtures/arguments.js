// What scripts can see of an arguments object: one kept as an array
// (strict functions, and sloppy ones with no named parameter to tie an
// argument to) and one mapped to the parameters. See tests/runtime.rs,
// arguments_objects_behave_the_same_however_they_are_stored.
(function () {
  function sloppy() { return arguments; }
  function mapped(a, b) { a = 'A'; arguments[1] = 'B'; return [arguments, a, b]; }
  function strict() { 'use strict'; return arguments; }
  const d = (o) => Reflect.ownKeys(o).map((k) => {
    const p = Object.getOwnPropertyDescriptor(o, k);
    const v = 'value' in p ? (typeof p.value === 'function' ? 'fn' : p.value) : 'accessor';
    return String(k) + ':' + v + ':' + [p.writable, p.enumerable, p.configurable].map(Number).join('');
  });
  const a = sloppy(1, 'x'), m = mapped(1, 2, 3), s = strict(1);
  let threw = false;
  try { s.callee; } catch (e) { threw = e instanceof TypeError; }
  const grown = sloppy(1);
  grown[3] = 'g';
  grown.length = 5;
  const getter = sloppy(1, 2);
  Object.defineProperty(getter, '0', { get: () => 'got', configurable: true });
  return [d(a), a.callee === sloppy, d(m[0]), m[1], m[2], Array.prototype.slice.call(m[0]),
    d(s), threw, Array.prototype.slice.call(a, 1), [...a], Object.prototype.toString.call(a),
    JSON.stringify(a), d(sloppy()), Array.from(grown), Object.keys(grown),
    Array.prototype.slice.call(getter), Math.max.apply(null, sloppy(3, 9, 4))];
})()
