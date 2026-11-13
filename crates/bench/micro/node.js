// Runs the language suite (language.js) under Node, for a V8 reference on
// the same machine as dasher's numbers:
//
//   node --jitless --single-threaded crates/bench/micro/node.js [filter]
//
// `--jitless` leaves V8 its interpreter (with inline caches) and no
// compilers, the fair comparison for page-load code; `--single-threaded`
// keeps GC on the main thread, so process CPU time is one thread's, as
// in dasher. Loops are sized and timed as `dasher-bench micro` does them:
// raised to at least 100 ms, then the median of 5 runs.

"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const file = path.join(__dirname, "language.js");
vm.runInThisContext(fs.readFileSync(file, "utf8"), { filename: file });

const RUNS = 5;
const MIN_MS = 100;
const filter = process.argv[2];

function cpuMs() {
  const u = process.cpuUsage();
  return (u.user + u.system) / 1000;
}

async function time(b, n) {
  const start = cpuMs();
  await b.fn(n);
  return cpuMs() - start;
}

(async () => {
  console.log("| benchmark | iterations | ns per op |\n| --- | ---: | ---: |");
  for (const b of globalThis.MICRO) {
    if (filter && !b.name.includes(filter)) continue;
    let n = b.n;
    const first = await time(b, n);
    if (first < MIN_MS) n = Math.ceil((n * MIN_MS) / Math.max(first, 0.5));
    const times = [];
    for (let r = 0; r < RUNS; r++) times.push(await time(b, n));
    times.sort((x, y) => x - y);
    const ns = (times[RUNS >> 1] * 1e6) / n;
    console.log(`| ${b.name} | ${n} | ${ns >= 100 ? ns.toFixed(0) : ns.toFixed(1)} |`);
  }
})();
