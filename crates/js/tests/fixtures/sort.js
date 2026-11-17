// Array.prototype.sort: see the test that includes this file.
(() => {
  let seed = 12345;
  const random = (below) => {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    return (seed >>> 8) % below;
  };
  // The definition, slowly: a stable insertion sort.
  const reference = (items, compare) => {
    const out = [];
    for (const item of items) {
      let at = out.length;
      while (at > 0 && compare(out[at - 1], item) > 0) at--;
      out.splice(at, 0, item);
    }
    return out;
  };
  const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
  const byKey = (a, b) => a.key - b.key;

  // Random arrays of every shape: lengths around the run sizes, few and
  // many distinct keys, sorted runs up and down, nearly sorted.
  let mismatches = 0, arrays = 0;
  for (const length of [0, 1, 2, 3, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 257, 1000]) {
    for (const keys of [1, 2, 7, 1000000]) {
      for (const shape of ['random', 'runs', 'down', 'nearly']) {
        const items = [];
        let key = random(keys);
        for (let i = 0; i < length; i++) {
          if (shape === 'random') key = random(keys);
          else if (shape === 'runs') key = random(40) === 0 ? random(keys) : key + random(3);
          else if (shape === 'down') key = random(40) === 0 ? random(keys) : key - random(3);
          else key = i + (random(20) === 0 ? random(keys) - i : 0);
          items.push({ key, i });
        }
        const sorted = items.slice().sort(byKey);
        if (!same(sorted, reference(items, byKey))) mismatches++;
        if (!same(items.toSorted(byKey), sorted)) mismatches++;
        arrays++;
      }
    }
  }

  // How many times the comparator is called.
  const calls = (items) => {
    let n = 0;
    items.sort((a, b) => (n++, a - b));
    return n;
  };
  const up = Array.from({ length: 1000 }, (_, i) => i);
  const twoRuns = up.slice(500).concat(up.slice(0, 500));
  const tenRuns = Array.from({ length: 1000 }, (_, i) => (i % 100) * 10 + Math.floor(i / 100));
  const shuffled = up.slice();
  for (let i = shuffled.length - 1; i > 0; i--) {
    const j = random(i + 1);
    [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
  }
  const counts = {
    sorted: calls(up.slice()),
    reversed: calls(up.slice().reverse()),
    appended: calls(up.concat([1000, 1001])),
    twoRuns: calls(twoRuns.slice()),
    tenRunsAtMost: calls(tenRuns.slice()) <= 1000 * 5,
    shuffledAtMost: calls(shuffled.slice()) <= 1000 * 10,
    allEqual: calls(new Array(1000).fill(7)),
  };

  // The comparator is called for two identical values too. A sort that
  // removes duplicates by noting them in its comparator, as jQuery's
  // uniqueSort does, depends on it.
  const duplicates = (() => {
    const a = { n: 1 }, b = { n: 2 };
    const items = [b, a, b, a, a, b];
    let seen = false;
    items.sort((x, y) => { if (x === y) seen = true; return x.n - y.n; });
    let n = 0;
    [a, a, a, a].sort(() => (n++, 0));
    return [seen, n, items.map((x) => x.n)];
  })();

  // A comparator that throws: the error comes out, nothing is lost.
  const thrown = (() => {
    const items = shuffled.slice(0, 200);
    let n = 0;
    try {
      items.sort((a, b) => { if (++n === 150) throw new RangeError('enough'); return a - b; });
    } catch (e) {
      return [e.name, n, same(items.slice().sort((a, b) => a - b), shuffled.slice(0, 200).sort((a, b) => a - b))];
    }
  })();

  // A comparator that contradicts itself: the sort ends, nothing is lost.
  const contradictory = (() => {
    const items = up.slice();
    items.sort(() => random(3) - 1);
    return same(items.slice().sort((a, b) => a - b), up);
  })();

  // A comparator that changes the array: what was there is what is sorted.
  const changed = (() => {
    const items = [5, 3, 4, 1, 2];
    items.sort((a, b) => { items.length = 0; items.push(99); return a - b; });
    return items;
  })();

  const holes = [3, , undefined, 1, , 2];
  holes.sort((a, b) => a - b);
  const arrayLike = { length: 4, 0: 'c', 1: 'a', 3: 'b' };
  Array.prototype.sort.call(arrayLike);
  const original = [3, 1, 2];
  const copy = original.toSorted();

  return {
    arrays,
    mismatches,
    counts,
    duplicates,
    thrown,
    contradictory,
    changed,
    holes: [holes.length, 4 in holes, 5 in holes, holes.slice(0, 4)],
    arrayLike,
    byDefault: [10, 9, 1, 'b', 'a', undefined, 'B', 100].sort(),
    answers: [
      [3, 1, 2].sort((a, b) => a > b),
      [3, 1, 2].sort((a, b) => (a > b ? '1' : '-1')),
      [3, 1, 2].sort(() => NaN),
      [3, 1, 2].sort((a, b) => (a < b ? -Infinity : Infinity)),
    ],
    stable: 'the quick brown fox jumps over the lazy dog and runs far away'.split(' ')
      .sort((a, b) => a.length - b.length).join(' '),
    toSorted: [original, copy],
    badComparator: (() => { try { [1, 2].sort(3); } catch (e) { return e.name; } })(),
  };
})()
