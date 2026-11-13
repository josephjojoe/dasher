// Array destructuring, spread, for-of, apply and concat (tests/runtime.rs,
// array_iteration_behaves_the_same_on_every_path). An array that uses the
// engine's own iterator gets it without the gets and the call, a spread
// of one is copied at once, apply copies a short array to the C stack,
// and concat and species creation take a plain array's elements and
// constructor directly (see PATCHES.md). Each case is also run with the
// iterator, `next`, `constructor` or Symbol.species replaced, so the
// general path is seen too. The answers are the same with and without
// the change. V8 differs in one way, before and after the change alike:
// QuickJS reads a Proxy's Symbol.iterator twice when spreading it into a
// call.
(function () {
  const out = [];
  const p = (x) => out.push(x);
  const log = [];
  function cases(tag) {
    const arr = [1, 2, 3, 4];
    const [a, ...rest] = arr;
    const [x, , y = 'def', z = 'dz'] = [10, 20, undefined];
    const [h1, h2, h3] = [, 'hole', ,];
    function three(q, r, s) { return [q, r, s, arguments.length].join('/'); }
    const sparse = [1, , 3];
    const longer = [1, 2]; longer.length = 4;
    const named = [5, 6]; named.extra = 1;
    let seen = [];
    for (const v of [7, 8, 9]) { seen.push(v); if (v === 8) break; }
    for (const v of sparse) seen.push(v);
    const grow = [1, 2];
    for (const v of grow) { if (grow.length < 5) grow.push(v * 10); seen.push(v); }
    const shrink = [1, 2, 3, 4];
    for (const v of shrink) { shrink.pop(); seen.push(v); }
    p([tag, a, rest.join(), rest.length, x, y, z, h1, h2, h3, three(...rest), three(...sparse), three(...longer),
      [...named].join(), [...'ab'].join(), [0, ...arr, ...[], 9].join(), seen.join()].join(' '));
    function sum() { let t = 0; for (let j = 0; j < arguments.length; j++) t += arguments[j]; return t + ':' + arguments.length; }
    const big = []; for (let j = 0; j < 40; j++) big.push(j);
    p([sum.apply(null, [1, 2, 3]), sum.apply(null, []), sum.apply(null, big), sum.apply(null, sparse), sum.apply(null, longer),
      sum.apply(null, { length: 2, 0: 'a', 1: 'b' }), sum.apply(null, null),
      (function () { return sum.apply(null, arguments); })(4, 5), Math.max.apply(null, [3, 9, 2]),
      Reflect.apply(sum, null, [1, 1]), new (function F() { this.v = Array.prototype.slice.call(arguments).join(); }).constructor(1)].join(' '));
    const mutator = [1, 2, 3];
    function mutate() { mutator.length = 0; mutator.push('m'); return Array.prototype.join.call(arguments); }
    p(mutate.apply(null, mutator) + ' ' + mutator.join());
    const parts = [[1, 2], [3], [4, 5, 6], [7]];
    const spreadable = { length: 2, 0: 'a', 1: 'b', [Symbol.isConcatSpreadable]: true };
    const notSpread = [8, 9]; notSpread[Symbol.isConcatSpreadable] = false;
    const c1 = [].concat.apply([], parts), c2 = [0].concat(sparse, longer, spreadable, notSpread, 'str', 5);
    p([c1.join(), c1.length, c2.length, JSON.stringify(c2), 1 in c2, Array.isArray(c1)].join(' '));
    class MyArray extends Array {}
    const mine = MyArray.from([1, 2]);
    p([mine.concat([3]) instanceof MyArray, mine.map((v) => v) instanceof MyArray, [1, 2].map((v) => v * 2).join(),
      [1, 2, 3].filter((v) => v > 1).join(), [1, 2, 3].slice(1).join(), [1, [2, [3]]].flat(2).join()].join(' '));
  }
  cases('plain');
  for (let round = 0; round < 2; round++) {
    const values = Array.prototype[Symbol.iterator];
    Array.prototype[Symbol.iterator] = function* () { log.push('custom iterator'); for (let j = 0; j < this.length; j++) yield this[j]; };
    cases('custom-iterator');
    Array.prototype[Symbol.iterator] = values;
    const ArrayIteratorPrototype = Object.getPrototypeOf([][Symbol.iterator]());
    const next = ArrayIteratorPrototype.next;
    ArrayIteratorPrototype.next = function () { log.push('custom next'); return next.call(this); };
    cases('custom-next');
    ArrayIteratorPrototype.next = next;
    const IteratorPrototype = Object.getPrototypeOf(ArrayIteratorPrototype);
    IteratorPrototype.return = function () { log.push('return called'); return {}; };
    cases('iterator-return');
    delete IteratorPrototype.return;
    const own = [1, 2, 3, 4];
    own[Symbol.iterator] = function* () { log.push('own iterator'); yield 'own'; };
    const [o1, ...oRest] = own;
    p(o1 + ' ' + oRest.join() + ' ' + [...own].join());
    const proxied = new Proxy([1, 2], { get(t, k) { log.push('proxy get ' + String(k)); return t[k]; } });
    p([...proxied].join() + ' ' + [].concat(proxied).length);
    const ctor = Array.prototype.constructor;
    Array.prototype.constructor = function () { log.push('custom constructor'); return []; };
    Array.prototype.constructor[Symbol.species] = undefined;
    cases('custom-constructor');
    Array.prototype.constructor = ctor;
    const species = Object.getOwnPropertyDescriptor(Array, Symbol.species);
    Object.defineProperty(Array, Symbol.species, { get() { log.push('custom species'); return Array; }, configurable: true });
    cases('custom-species');
    Object.defineProperty(Array, Symbol.species, species);
    const withCtor = [1, 2]; withCtor.constructor = MyArrayLike; function MyArrayLike() { log.push('own constructor'); return []; }
    MyArrayLike[Symbol.species] = MyArrayLike;
    p(String(withCtor.concat([3]).length));
    Object.defineProperty(Array.prototype, 1000, { get() { log.push('proto index'); return 'from proto'; }, configurable: true });
    const reach = [1, 2]; reach.length = 1002;
    cases('proto-index');
    p([...reach].slice(999).join() + ' ' + [].concat(reach).length + ' ' + (function () { return arguments.length; }).apply(null, reach));
    delete Array.prototype[1000];
    cases('plain-again');
  }
  p(log.join());
  return out;
})()
