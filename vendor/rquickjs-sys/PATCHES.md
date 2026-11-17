# dasher's changes to rquickjs-sys

This directory is `rquickjs-sys` 0.11.0 from crates.io, which carries
QuickJS-NG 0.11.0, with the changes listed here. The workspace's
`[patch.crates-io]` entry points cargo at it.

Every change in the C source is marked with a comment that starts
`dasher:`, so this lists them:

```sh
grep -n 'dasher:' build.rs quickjs/*.c quickjs/*.h
```

and this shows them in full:

```sh
diff -ru ~/.cargo/registry/src/*/rquickjs-sys-0.11.0 vendor/rquickjs-sys
```

To move to a newer rquickjs, copy the new `rquickjs-sys` here and apply
the changes again. The licence is upstream's: MIT, in
[`quickjs/LICENSE`](quickjs/LICENSE).

The changes share one rule. A fast path is taken only when what it skips
cannot be seen by a script; otherwise the engine's own path runs
unchanged. Each change a script could notice has a test in
`crates/js/tests` that runs the same script both ways, or against output
recorded from the engine without the change.

This file says what each change is. The measurements each was accepted
on are in the commit that made it (`git log -- vendor/rquickjs-sys`).

**Depending on dasher from another workspace.** A `[patch]` section only
applies in the workspace that declares it. `dasher-js` calls functions
that exist only in this copy of the engine (`JS_SetMallocFunctions`,
`JS_SetScriptSource` and others below), so a workspace that depends on
dasher must carry the same patch, pointing at this directory of a
checkout or at the repository:

```toml
[patch.crates-io]
rquickjs-sys = { git = "https://github.com/josephjojoe/dasher" }
```

## Build

- **`build.rs` rebuilds when the C source changes**
  (`cargo:rerun-if-changed=quickjs`). Without it cargo keeps the engine
  it built first, whatever is edited here.
- **No dump code.** Upstream defines `ENABLE_DUMPS` whenever `NDEBUG` is
  unset, and dasher never sets `NDEBUG`, to keep the assertions. With the
  dump code every string carries a 16-byte link in a list of all strings.
  Here `ENABLE_DUMPS` is not defined by default; build with
  `-DENABLE_DUMPS` to get the dumps back.

## Memory

- **One copy of a script's text.** The engine keeps each function's
  source for `Function.prototype.toString`. Upstream copies it per
  function, and functions nest, so a bundle's text is stored once per
  level of nesting. Here a script's text is held once, in a counted
  `JSSourceText` that each function compiled from it points into, and is
  freed with the last of them. The cost: a script's whole text stays for
  as long as any one of its functions does.
- **`JS_SetMallocFunctions(rt, mf, opaque)`** (also in `quickjs.h`)
  replaces the allocator functions a runtime was made with. rquickjs
  reaches an allocator through a boxed trait object, three calls for
  each `malloc`; `crates/js/src/heap.rs` makes the runtime that way and
  then points the engine straight at dasher's slab heap.
- **A bound function without its record can be freed.** When
  `js_function_bind()` cannot allocate the record of what it binds, it
  frees the function object, whose finalizer and mark function read
  through the record that is not there: a segmentation fault, which a
  page reached at its memory limit. Both now return when there is no
  record. Test: `crates/js/tests/limits.rs`.
- **A closure keeps the suspended function it reads from.** A generator
  or async function keeps its frame on the heap, and a closure made in
  it reads the frame's variables in place. Upstream's collector did not
  know of that: when the function was garbage in a cycle and the closure
  was not, it freed what the variables held, and the function's
  finalizer then handed the freed values to the closure. The memory was
  reused, and a later collection stopped on `gc_decref_child`'s
  assertion, or the closure read another object. A page reached it with
  an abandoned async function (apnews.com). Here a reference to a
  variable in such a frame holds a reference to the frame's owner (the
  generator object or the async function's data) until the variable
  leaves the frame, and is one of the collector's objects from then on,
  so a cycle through the frame is still collected. The cost: such a
  closure keeps the whole suspended frame, not only the variables it
  reads, and `JSVarRef` is 8 bytes larger. Bellard's QuickJS does the
  same, and QuickJS-NG's main branch does since 0.11.0, so this change
  goes away with a newer rquickjs. Test: `crates/js/tests/gc.rs`.

## Compiling

- **Functions are compiled on their first call.** Upstream compiles
  every function of a script before any of it runs, and most of a page's
  functions never run. Here a function not likely to run at once is
  parsed in full, so that a syntax error anywhere still stops the whole
  script, then dropped, and compiled from the script's text when it is
  first called (`js_lazy_compile`). Constructors, class static blocks,
  functions in a `with`, functions written in parentheses or after `!`
  (taken to be called at once, as V8 takes them) and functions that call
  `eval` directly are compiled with their parent. Line and column
  numbers, `toString`, `length` and `name` come out the same.
  `JS_SetLazyFunctions(rt, false)` turns it off. Test:
  `fixtures/lazy.js`.
- **Compiled scripts can be kept by the embedder.** For dasher's script
  cache (`crates/js/src/script_cache.rs`): `JS_SetScriptSource` describes
  a script whose text the embedder owns, so pages share one copy outside
  their heaps; `JS_WriteObject` with `JS_WRITE_OBJ_SCRIPT` writes
  functions not yet compiled as they are, with text as offsets into the
  script; `JS_ReadObject` with `JS_READ_OBJ_SCRIPT` reads them back
  against that text and the caller's file name, checking every offset
  and count; `JS_SetLazyCompileHook` tells the embedder which functions
  ran, and `JSScriptSource.hints` asks for those to be compiled with the
  script next time. One upstream bug is fixed on the way: a template
  object read back was not frozen. Test: `crates/js/tests/script_cache.rs`.
- **The parser remembers what its bracket lookahead found.** At `(`, `[`
  and `{` the parser reads ahead to the matching bracket to tell an
  arrow function from an expression and a pattern from a literal, and
  brackets nest, so upstream reads deeply nested code many times. Here
  the answer for each bracket is kept. The lookahead also stops as soon
  as it knows, and makes no atoms or strings for the tokens it passes.
  Test: `fixtures/lookahead.js`.
- **Parser and compiler throughput.** Short decimal numbers, plain ASCII
  names and plain strings are read in the tokenizer itself; binary
  operators are parsed by precedence climbing where upstream recursed
  once per precedence level; bytecode is written in place when the
  buffer has room (`cutils.h`); the compiler's buffers and tables start
  with room; `find_var` follows its hash table instead of scanning.
  Test: `fixtures/parser.js`.

## Running

- **`stack` is written out when it is first read.** Upstream formats an
  error's `stack` when the error is made or thrown. Pages throw and
  catch far more errors than they read stacks of.
- **Closures, `arguments` objects, C functions, RegExp objects and
  `exec` results are made from a known shape**, as upstream makes
  arrays, where upstream defined their properties one at a time. A
  sloppy `arguments` with nothing to map is kept as a fast array.
  Tests: `fixtures/closures.js`, `fixtures/arguments.js`.
- **Object literals are made from the shape their site ended with**
  (`OP_object_site`, with two opcodes in `quickjs-opcode.h`), for
  literals of plain `name: value` properties. Test:
  `fixtures/object_literals.js`.
- **Spread, object rest and `Object.assign` copy an ordinary source in
  bulk**; `for-in` and `Object.keys` read an ordinary object's shape.
  Tests: `fixtures/copying.js`, `fixtures/enumeration.js`.
- **Arrays skip the iterator protocol when it is the engine's own**:
  spread, destructuring and `for-of` over a plain array whose iterator
  nobody has replaced. Test: `fixtures/array_iteration.js`.
- **`i++` on a `let` stays an integer add**, as it already did on a
  `var`. Test: `fixtures/increments.js`.
- **C functions are called without entering the interpreter**, and
  promises and async functions allocate less per step: a capability for
  the realm's own `Promise` is made without its executor function.
  Test: `fixtures/promises.js`.
- **`Map` and `Set` mix their hash**, since small integers and aligned
  addresses otherwise share buckets, and compare integer and object
  keys inline. Test: `fixtures/collections.js`.
- **Reference counts are tested inline.** `JS_FreeValue`, `JS_DupAtom`
  and their relatives are exported, and the library is built `-fPIC`, so
  the compiler could not inline them into the engine's own 2,000 calls.
  `quickjs.c` now uses inline versions; the exported functions are
  unchanged for callers outside it.
- **The cycle collector marks children directly**, with a copy of
  `mark_children` per pass and inlined marking of arrays, `arguments`
  objects and bytecode functions. It does the same work in the same
  order.

- **A function's body shares its parameters with the closures in their
  default values.** When a parameter has a default value or a pattern,
  the parameters are variables of a scope of their own, which closures
  in those expressions read. Upstream left the body with the arguments
  themselves, a second copy: `function f(a = 1, g = () => a) { a = 2;
  return g(); }` returned 1. Here the body uses the scope's variable,
  unless it declares the name again with `var` or as a function, which
  by the specification is a second variable that starts from the
  parameter's value. The code a direct `eval` in the body compiles
  finds the same variable: the compiled function records which names
  the body left to the parameters (`JSVarDef.has_param_var`, written
  with the bytecode). A `var` that such an `eval` declares belongs to
  the body, so it hides the parameter from the body and not from the
  default values: the object holding those variables is looked in
  before the parameter. Both upstreams still have the two copies.
  Test: `crates/js/tests/parameters.rs`, against Node's answers.
- **`var` in an `eval` leaves an existing variable alone.** Inside a
  function, upstream set a variable to `undefined` each time code run
  by a direct `eval` declared it, so a second `eval('var a')` lost what
  the first had stored. Test: `crates/js/tests/eval_var.rs`, against
  Node's answers.

## Strings, JSON, arrays

- **`s += x` appends in place** when the variable holds the only other
  reference to the string.
- **`JSON.parse` reads the string itself** instead of converting it to
  UTF-8 and building every key through a string buffer, and
  **`JSON.stringify` writes plain objects from their shape**. Test:
  `fixtures/json.js`, against `json.expected.json`.
- **String search, split, replace and case conversion** use `memchr`
  and `memcmp` where the strings' widths allow. Test:
  `fixtures/strings.js`, against `strings.expected.json`.
- **An atom's index is kept in its string**, so `obj[key]` with a key
  that is already an atom does not walk a hash chain.
- **`Array.prototype.sort` is a natural merge sort.** Upstream's
  quicksort makes about n log n calls of the comparator whatever it is
  given; this finds the runs already there, so a sorted or reversed
  array costs n - 1 calls. It is stable, and like V8 it moves an element
  only on a negative answer, so a comparator that answers `true` or
  `false` leaves the array as it is. **The comparator is called for two
  identical values**, where upstream answers 0 for them itself: jQuery's
  `uniqueSort` removes duplicates only if its comparator was handed the
  same element twice, so without the call `$(items).parent()` keeps one
  copy of the parent per item. Test: `fixtures/sort.js`, against
  `sort.expected.json`.

## Regular expressions

In `quickjs.c`, `libregexp.c` and `libregexp.h`. Every path below is
behind `lre_dasher_fast`; `lre_set_fast_paths(false)` gives the engine's
own way, and `crates/js/tests/regexp.rs` runs its patterns through every
method both ways and expects the same answers.

- **A pristine RegExp skips the spec's steps.** While nothing the steps
  would look at has been changed (`js_regexp_pristine()`: no own
  properties but `lastIndex`, the realm's prototype, and `exec`, `flags`,
  the flag getters and `Symbol.species` as the engine made them),
  `test`, `replace`, `match`, `split` and `search` run the matcher
  directly and build only what they return.
- **`new RegExp(source, flags)` reuses bytecode**: the last 32 patterns
  compiled are kept.
- **A pattern that is only characters is a string search**
  (`lre_exec_literal()`), and never starts the interpreter.
- **A greedy repeat of one character test** (`\d+`, `[^"]*`, `.*`) runs
  as a loop, and when it backs off it passes over lengths at which the
  rest of the pattern fails its first test.
- **A hook for the loop of a search** (`lre_set_search_hook()`). A
  regexp without the sticky flag tries each position in turn. dasher's
  hook (`crates/js/src/regexp.rs`) reads a pattern's bytecode into a
  plan and skips positions that cannot match: those with no character
  every match needs still ahead, those inside a run a leading repeat
  already failed on, and those that do not start with two characters a
  match can begin with.

Catastrophic patterns still stop at the page's deadline: the new loops
poll for it or are bounded by the subject's length
(`crates/js/tests/limits.rs`).
