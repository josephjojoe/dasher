# dasher's changes to rquickjs-sys

This directory is `rquickjs-sys` 0.11.0 from crates.io (QuickJS-NG 0.11.0)
with the changes below. The workspace's `[patch.crates-io]` entry points
cargo at it. Every change in the C source is marked with a `dasher:`
comment, so `grep -n 'dasher:' quickjs/*.c` lists them, and

```sh
diff -ru ~/.cargo/registry/src/*/rquickjs-sys-0.11.0 vendor/rquickjs-sys
```

shows them in full. To move to a newer rquickjs, copy the new
`rquickjs-sys` here and apply the changes again; each is small and
self-contained. `crates/js/tests/runtime.rs` has a test for each change
that scripts could notice.

## `build.rs`: rebuild when the C source changes

Adds `cargo:rerun-if-changed=quickjs`. Without it cargo keeps using the
engine it built first, whatever is edited here.

## `quickjs/quickjs.c`: one copy of a script's text

The engine keeps each function's source text for `Function.prototype.toString`.
Upstream copies it per function, and function sources nest: a script wrapped
in one function that defines a thousand others is stored about twice, and a
bundle with five levels of nesting five times.

Here a script's text is copied once, when its first function is parsed, into
a counted `JSSourceText`. Each `JSFunctionDef` and `JSFunctionBytecode` holds
a reference and a pointer into it (`js_parse_set_source`), and the text is
freed with the last function compiled from it. Functions read back with
`JS_ReadObject` still own their text (`source_text == NULL`).

The cost is that a script's whole text stays for as long as any one of its
functions does, where upstream would keep only that function's part.

Measured on 14 recorded real pages (macOS, arm64, against the same build
with a copy per function): 7% less peak memory summed over the pages
(1224 to 1134 MiB), 11 to 14% less on the pages built from large bundles,
and CPU time within 2%.

## `quickjs/quickjs.c`: `stack` is written out when it is first read

Upstream writes an error's `stack` string when the error is created or first
thrown: up to ten frames, each with a `printf`, a name converted to UTF-8 and
a line number looked up. Pages throw and catch errors without looking at the
stack far more often than they read it, and some do so thousands of times.

Here `build_backtrace` notes where the frames are (`JSBacktrace`: for each
frame the function's name string, a reference to its bytecode and the
program counter) and defines `stack` as an auto-init property
(`JS_AUTOINIT_ID_BACKTRACE`). The first read runs `js_backtrace_autoinit`,
which writes the same text upstream would have. The auto-init id grows from
two bits to three to make room for the new kind.

Three cases keep upstream's direct path: `Error.prepareStackTrace` is set,
the error is a syntax error with a source position, or the call is
`Error.captureStackTrace`.

`build_backtrace` also returns at once for a value that cannot be given a
stack: something that is not an error, or an error that already has one and
is being thrown again. Upstream wrote a full stack for these and threw it
away. One thing follows from that: with `Error.prepareStackTrace` set,
throwing an error a second time no longer calls it again.

`ctx->error_back_trace`, which only `js_std_cmd` reads (and nothing in
rquickjs or dasher calls that), is now `null` instead of the stack text.

Measured on the same pages: 3% less CPU time summed over the pages, 14%
less on the page that throws most (6.99 to 6.00 s), and no change in
memory. A script that compares `stack` strings for some forty kinds of
error, and `toString()` of every kind of function, gives the same output
with and without these two changes.

## `quickjs/quickjs.c`: the parser remembers what its bracket lookahead found

At `(`, `[` and `{` the parser cannot tell an arrow function from a
bracketed expression, or a literal from a destructuring pattern, until it
sees the token after the matching bracket. `js_parse_skip_parens_token`
reads ahead to find it and rewinds. Brackets nest, so upstream reads the
inside of a bracket once for every bracket around it: on six recorded
pages, with 30 million tokens to parse, the lookahead read 74 million.

Here a scan notes, for each bracket it passes, what a scan starting there
would return (`JSSkipMemo`, a table on the `JSParseState`, freed when
parsing ends). A later scan starting at that bracket returns it at once.
Only brackets with at least 64 bytes inside are noted, which keeps the
table to 512 KiB on the largest script measured; shorter ones are cheap
to read again.

What a scan finds depends on how identifiers become keywords, which varies
with the function being parsed (strict mode, `yield`, `await`). An entry is
used only under the rules it was recorded under (`js_skip_memo_ctx`, which
has to stay in step with `update_token_ident`).

Measured on 14 recorded real pages (macOS, arm64, five runs, against the
same build without this change): 5% less CPU time summed over the pages
(24.4 to 23.2 s), 8% less on two pages built from large bundles and 13%
less on one, and no change in memory. On the Linux musl build it is 4% less
(32.1 to 30.9 s), again with no change in memory. The lookahead's reads fall from 2.5
to 1.3 for each token parsed, and compiling a corpus of the pages' scripts
goes from 13 to 17 MiB of source a second. With every remembered answer
also checked against a fresh scan, 366,000 answers over those pages and
scripts all agreed.

## `quickjs/quickjs.c`: closures and `arguments` objects are made from a known shape

The engine keeps the shape of a new array (`ctx->array_shape`) so that
making one is an allocation and a store. A closure and an `arguments`
object are made nearly as often, and upstream builds each by defining its
properties one at a time through the general path: `length` and `name`,
and `prototype` for a constructor; or `length`, `Symbol.iterator` and
`callee`. Each definition looks up the next shape and may resize the
object's property array.

Here the context keeps those shapes too (`closure_shape`,
`arguments_shape`). The first closure of each kind and the first
`arguments` object are built upstream's way and their shape is noted.
After that `js_closure` and `js_build_arguments` allocate the object with
that shape and store the property values in place. A generator function,
whose `prototype` is an object made on the spot, keeps upstream's path.

## `quickjs/quickjs.c`: a sloppy `arguments` with nothing to map is kept as an array

Upstream gives every sloppy function a mapped `arguments` object: each
argument is an ordinary property, and those with a named parameter are
tied to that parameter's variable. `apply`, spread and
`Array.prototype.slice` then read the arguments one property lookup at a
time. A strict function gets the unmapped object, which keeps the
arguments in an array those three read directly.

When no argument has a named parameter to be tied to (the function
declares none, or was called with none) the two objects differ only in
`callee`. Here such a call gets the unmapped object with `callee` set to
the function, as the mapped one has it. This is the
`function () { return f.apply(this, arguments); }` of every bundle.

Measured, with the change above: making a closure in a loop takes 30%
less time (460 to 320 ms for three million) and a loop of
`inner.apply(null, arguments)` and `Array.prototype.slice.call(arguments, 1)`
half the time (560 to 270 ms). A script that reads every own property, attribute,
enumeration order and array method result of 16 kinds of `arguments`
object and 20 kinds of function (659 answers) gives the same output with
and without the two changes, and V8 gives the same apart from the wording
of error messages.

## `quickjs/quickjs.c`: `Map` and `Set` mix their hash

A `Map` picks a bucket from the low bits of its key's hash
(`h & (hash_size - 1)`). Upstream's hash of a number is its bits times a
constant, and of an object its address times a constant. A small integer
stored as a double has zeros in its low bits and an address is aligned,
so the low bits of both barely vary: integer keys all landed in one
bucket and object keys in one bucket of sixteen, and every lookup walked
a list.

Here `map_hash_key` passes upstream's hash through a 32-bit mixer before
it is used. Order is unaffected: a `Map` iterates in insertion order
whatever its buckets hold.

Measured: a `Set` of 500 integers queried 1.5 million times, 1080 to
60 ms; a `Map` of 500 objects likewise, 210 to 70 ms; a `WeakMap` of 2,000
objects, 460 to 130 ms.

## `quickjs/libregexp.c`: a hook for the loop of a search

A regexp without the sticky flag is compiled with a loop in front of the
pattern: try here, else skip a character and try again. The interpreter
runs that loop as part of the pattern, saving a backtracking state at
every position and starting the pattern at each one.

`lre_set_search_hook` lets the embedder run that loop. When a hook is
set, `lre_exec` passes it the pattern's bytecode (after the loop) and the
subject, and the hook calls `lre_search_try` at each position it wants
tried. The matcher is not changed. dasher's hook is
`crates/js/src/regexp.rs`: it reads the start of the bytecode to find the
characters a match can begin with, or that the pattern begins with `^`,
and tries only the positions that qualify. A static assertion beside the
hook names every opcode that module reads, so an engine update that
renumbers them does not compile.

Measured: three patterns tested 90,000 times each against a
110-character user-agent string (a literal near its end, a literal that
is absent and a case-insensitive alternation), 890 to 120 ms; a mix of
five regexp operations taken from real pages, 770 to 350 ms. `crates/js/tests/regexp.rs` runs 51
patterns over 19 subjects through `exec`, `test`, `search`, `replace`,
`split`, `match` and `matchAll` with the hook and without and expects the
same answers; V8 gives the same answers.

## What the four changes above measure together

On the 14 recorded pages (macOS, arm64, three runs): 4% less CPU time
summed over the pages (20.3 to 19.6 s), 8% less on the page that gains
most, and no change in memory. The search hook on its own is under 1%,
and so is the `Map` hash: the patterns and keys they help are rarer on
these pages than in the loops above.

## `quickjs/quickjs.c`: `i++` on a `let` stays an int add

`for (let i = 0; i < n; i++)` compiles `i++` as `get_loc_check i;
post_inc; put_loc_check i; drop`. The optimizer turns `post_inc put_loc
drop` into `inc put_loc`, but not the `_check` form a `let` gets, and
`OP_post_inc` had no fast path: every `i++` converted its operand with
`JS_ToNumericFree` and added through `js_unary_arith_slow`. An assignment
statement to a `let` likewise kept its `dup put_loc_check drop`.

Here `OP_post_inc` and `OP_post_dec` add or subtract an int in place
(the general path keeps the int limits, which become doubles), and
`resolve_labels` rewrites `post_inc put_loc_check(n) drop` to `inc
put_loc_check(n)` and `dup put_loc_check(n) drop` to `put_loc_check(n)`.
The TDZ check stays.

Measured in the micro-benchmarks (standalone `qjs` built from this
source, Graviton4): the function-call loop 48.5 to 39.0 ns per
iteration, the integer loop 31.5 to 21.8 ns, a method call 75 to 65 ns.
`crates/js/tests/fixtures/increments.js` applies `++` and `--` to 22
kinds of value, at the int limits and in the TDZ, and gives the same
answers with and without the change and under V8.

## Fast paths for calls, object creation, copying, iteration and async

The changes below share a rule: each fast path is taken only when what
it skips cannot be seen by a script, and otherwise the general path runs
unchanged. Each has a differential script in `crates/js/tests/fixtures/`
run by `crates/js/tests/runtime.rs`; the expected output in the test is
what the engine gave before the change, and each script was also run
under V8 (`node`), with the differences noted in the script, all of
which QuickJS had before. test262 (tc39/test262 at its 2026-11-13 head,
41,841 tests, built with assertions on) fails the same 118 tests with
and without them.

Times are from a standalone `qjs` built from this source (gcc `-O2`,
glibc `malloc`) on the Graviton4 the micro-benchmarks ran on, before and
after each change, on the loops of `crates/bench/micro/language.js`; the
dasher figures for all of them together are at the end. The machine was
shared with other builds, so single figures carry a few percent of
noise.

One thing to know when changing any of them: a shape that is not hashed
belongs to one object and is changed in place, and `add_property()`
asserts that nothing else references it. Only hashed shapes may be held
or shared (`object_shapes`, `c_function_shape`, the for-in iterator,
the source of a bulk copy). Holding an unhashed one crashed two real
pages before this was caught; the `qjs` used for differential runs must
be built without `NDEBUG` to see it.

### `quickjs/quickjs.c`: object literals are made from the shape their site ended with

A literal whose properties are all `name: value` or `name` (no spread,
computed key, method, accessor or `__proto__`) is compiled with a site
number: `OP_object_site` at the start and `OP_object_site_end` at the
end, through the temporary opcodes `object_start` and `object_end`
(new opcodes go after all others in `quickjs-opcode.h`, so no opcode is
renumbered and the precompiled builtins stay valid). The first time a
site's literal ends, the shape of its object is noted in
`JSFunctionBytecode.object_shapes`, if it is a hashed shape with
Object.prototype and only configurable, writable, enumerable data
properties. After that `OP_object_site` allocates the object with that
shape and every value undefined, and `OP_define_field` stores into a
property that is already there and is configurable, writable and
enumerable, which is all a definition with those attributes does to it.
Until the literal ends only its frame can reach the object, so its
property order and attributes are all a script can see of the change,
and those are the same. Class fields go through the same
`OP_define_field`.

Upstream each literal cloned the empty shape and grew it one property
at a time, reallocating the shape and the property array, because the
intermediate shapes die with their object. Object literal loop: 287 to
108 ns. `fixtures/object_literals.js`.

### `quickjs/quickjs.c`: spread, object rest and Object.assign copy an ordinary source in bulk

`js_copy_data_properties_fast()` handles a source that is an ordinary
object with a hashed shape and no array-index key: it holds the shape,
walks it for the enumerable string keys and then the symbols (the order
`JS_GetOwnPropertyNamesInternal` gives), reads each value from its slot
while the source still has that shape and with a get once it does not,
and defines or sets each on the target. That is what the general path
does without the key list it allocates and the lookup per key; like
upstream it reads a key that a getter deleted during the copy as
undefined. An empty ordinary target with the source's prototype,
copying a source of plain data properties, takes the source's shape and
values whole; for Object.assign only if nothing on the prototype chain
has any of the keys. Spread and Object.assign loop: 908 to 627 ns.
`fixtures/copying.js`.

### `quickjs/quickjs.c`: arrays skip the iterator protocol when it is the engine's

`js_array_iterator_untouched()` says whether iterating an array would
use the engine's iterator: a fast array with no own Symbol.iterator,
whose prototype is the realm's Array.prototype, whose Symbol.iterator is
a data property holding the original `values`, with
%ArrayIteratorPrototype%.next a data property holding the original. It
does the lookups a get would do. Then:

- `for_of_start` (for-of, array destructuring, `yield*`) makes the
  iterator directly instead of getting Symbol.iterator, calling it and
  getting `next`. The iterator is still a real one, so `return` and
  everything else behave as before.
- `append` (spread into an array or a call) copies the elements at once
  into the array being built, and makes no iterator: nothing of it could
  be seen.
- `define_array_el` adds the next element of a fast array in place, as
  `JS_CreateProperty` would.
- `apply` with a fast array of up to 16 elements copies them to the C
  stack instead of an allocated list, still duplicating each.
- `concat` copies a fast array's elements at once into a fast result,
  and `JS_ArraySpeciesCreate` makes a plain array directly when the
  `constructor` and Symbol.species gets would find the realm's Array
  and its original species getter.
- `map` sizes its result once and `map` and `filter` add each result in
  place (`js_array_append_fast()`).

Destructuring and rest/spread loop: 1252 to 898 ns; concat.apply: 942
to 601; apply and arguments: 369 to 289; map/filter/reduce: 2231 to
1767. `fixtures/array_iteration.js`, which also replaces the iterator,
`next`, `constructor` and Symbol.species and puts an index on
Array.prototype to see the general path.

### `quickjs/quickjs.c`: functions are named and made without definitions

`OP_set_name` stores the name when the function's `name` is still the
empty string, only configurable, that a new function is made with. And
the context keeps `c_function_shape`, the shape `length` and `name` give
a function made in C (`js_function_set_properties()`: C functions,
C function data, promise resolving functions), noted the first time and
then taken by every such function with its two values stored in place.
Create and call a closure: 183 to 172 ns.

### `quickjs/quickjs.c`: promises and async functions allocate less per step

- A promise capability for the realm's own Promise is made without the
  executor function (`js_promise_new_capability_direct()`): the executor
  is the engine's and only keeps the two functions it is given, and
  Promise's `prototype` cannot change. Promise.prototype.then,
  Promise.resolve and the rest take it.
- An async call keeps the promise it returned and settles it with
  `js_promise_settle()`, the body of a resolving function, instead of
  making the two resolving functions nobody else can reach.
- The two functions an await passes to the awaited promise are made at
  the first await of a call and reused by every later one
  (`JSAsyncFunctionData.resume_funcs`); they only ever go to
  `perform_promise_then()` on an engine promise.

Promise chain loop: 977 to 629 ns; async/await: 994 to 384.
`fixtures/promises.js` records the order in which some forty reactions
run, which V8 gives the same.

### `quickjs/quickjs.c`: for-in and Object.keys read an ordinary object's shape

For an ordinary object of plain data properties with string keys and a
hashed shape, the for-in iterator takes the object's shape for its own
key list instead of adding each key to itself, and `js_for_in_next()`
skips the check that a key still exists while the object keeps that
shape. Ordinary prototypes are checked for enumerable keys from their
shapes instead of an allocated key list. `Object.keys`,
`Object.values` and `Object.getOwnPropertyNames` of such an object fill
an array of the right size from the shape. for-in and Object.keys loop
(dasher): 1326 to 594 ns. `fixtures/enumeration.js`.

### `quickjs/quickjs.c`: C functions are called without entering the interpreter

`JS_CallInternal` is now a small inline function that sends anything
but bytecode to its class's call handler, with the interrupt check and
the not-a-function error upstream makes first, and calls the interpreter
(`JS_CallInterpreter`) only for bytecode. Upstream every call of a C
function, a DOM getter or a bound function went through the
interpreter's entry, which saves every callee-saved register and sets up
a frame of over a kilobyte. A loop of `Math.abs` and `indexOf` calls:
1.19 to 1.04 s; JS-to-JS calls unchanged.

### `quickjs/quickjs.c`: Map and Set compare int and object keys inline

`map_find_record()` decides two ints, or two objects or symbols, by
their bits, which is what SameValueZero gives for those pairs, and calls
`js_same_value_zero()` for every other pair. Map/Set loop: 241 to 193
ns. `fixtures/collections.js` gained ints against the same numbers as
doubles, -0, NaN and symbols.
