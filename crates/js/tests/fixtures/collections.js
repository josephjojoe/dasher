// Map, Set and WeakMap keyed by small integers, other numbers and
// objects: insertion order and lookups. See tests/runtime.rs,
// map_and_set_keys_keep_their_order_and_identity.
(function () {
  const m = new Map();
  for (let i = 0; i < 40; i++) m.set(i * 7 % 40, i);
  m.delete(3);
  m.set(3, 'again');
  m.set(-0, 'zero');
  m.set(NaN, 'nan');
  m.set('3', 's');
  m.set(2 ** 31, 'big');
  m.set(1.5, 'f');
  const objs = Array.from({ length: 64 }, (_, i) => ({ i }));
  const s = new Set(objs);
  for (let i = 0; i < 64; i += 3) s.delete(objs[i]);
  const w = new WeakMap(objs.map((o, i) => [o, i]));
  return [[...m.keys()].join(','), m.get(0), m.get(NaN), m.get('3'), m.get(3), m.get(2147483648),
    m.get(1.5), m.size, s.size, [...s].map((o) => o.i).join(','),
    objs.every((o, i) => w.get(o) === i), s.has(objs[1]), s.has(objs[0]), s.has({})];
})()
