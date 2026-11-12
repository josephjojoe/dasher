// What scripts can see of a function made from a cached shape
// (tests/runtime.rs, closures_have_the_properties_scripts_expect): its
// own properties, in order, with their values and attributes.
(function () {
  function decl(a, b) {}
  const arrow = (a) => a;
  const method = ({ m(a, b, c) {} }).m;
  async function af() {}
  function* gen() {}
  const d = (f) => Object.getOwnPropertyNames(f).map((k) => {
    const p = Object.getOwnPropertyDescriptor(f, k);
    const v = p.value === null ? 'null' : typeof p.value === 'object' ? 'object' : p.value;
    return k + ':' + v + ':' + [p.writable, p.enumerable, p.configurable].map(Number).join('');
  });
  const fs = [];
  for (let i = 0; i < 3; i++) fs.push(function (x) { return i; });
  fs[0].extra = 1;
  delete fs[1].name;
  fs[2].prototype = null;
  return [d(decl), d(arrow), d(method), d(af), d(gen), fs.map(d), fs.map((f) => f()),
    decl.prototype.constructor === decl, new decl() instanceof decl,
    fs[0].prototype !== fs[1].prototype, Object.getPrototypeOf(af) !== Function.prototype];
})()
