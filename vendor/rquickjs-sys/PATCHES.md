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
site's literal ends, `OP_object_site_end` rewrites itself into
`OP_object_site_note`; the second time, the shape of its object is
noted in `JSFunctionBytecode.object_shapes`, if it is a hashed shape
with Object.prototype and only configurable, writable, enumerable data
properties. Most literal sites of a large bundle run once, and noting
the shape at the first end (a table slot and a shape kept for each)
cost 3 to 4 MiB of peak RSS on zillow.com; the table's count sits in
what was padding, so each function's bytecode grows by 8 bytes, the
table pointer. After that `OP_object_site` allocates the object with that
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

### What these changes measure together

In dasher (`dasher-bench micro`, Graviton4, median of 5 runs, against the
same build with the engine source before them): function call 57.7 to
46.3 ns, integer loop 36.0 to 25.1, object literal 267 to 111, spread and
Object.assign 904 to 524, destructuring and rest/spread 1284 to 877,
apply and arguments 391 to 301, concat.apply 865 to 518, Map/Set 284 to
224, promise chain 1383 to 1082, async/await 817 to 329, for-in and
Object.keys 1317 to 552, and native DOM getters such as `nodeType` 89 to
73. On the thirteen heavy recorded pages, five runs of each build
alternating: 6.7% less CPU time summed over the pages (38.70 to
36.10 s), 4.8 to 12.8% less on nine of them, and peak RSS within the
run-to-run spread (1252 to 1254 MiB summed). The tables are in
`crates/bench/README.md`, "Language fast paths".

## `quickjs/quickjs.c`: functions are compiled on their first call

Upstream compiles every function in a script before any of it runs. Most
of a page's functions are never called: on bbc.com/news 15,000 of 52,637
ran. Here a function that is not likely to run at once is parsed in full
and then dropped, and compiled again from the script's text the first
time it is called. `JS_SetLazyFunctions(rt, false)` turns this off.

**What is parsed, and when.** The parser still reads every function in
full (`js_parse_function_decl2`), so a syntax error anywhere stops the
whole script, as the spec requires. At the end of a function that is a
candidate (`js_lazy_begin`) it is dropped (`js_lazy_drop`): what its
parent needs is kept in a *shell*, a `JSFunctionDef` with nothing in it,
and what compiling it later needs in a `JSLazyRecord`. When the parent is
compiled the shell becomes a `JSFunctionBytecode` with no code
(`js_create_lazy_function`, `is_lazy`), which closures are made from as
usual. Its first call (`JS_CallInternal`, `async_func_init`) compiles it
(`js_lazy_compile`) from the script's shared `JSSourceText` and fills in
the same `JSFunctionBytecode`. Bytecode is written out compiled:
`JS_WriteObject` compiles what has not been, so dasher's glue stays fully
compiled.

A record also holds the records of the functions inside the function, so
compiling it skips over each of them (resuming the tokenizer at the token
after it) instead of parsing it again, and makes it a lazy function in
turn. A function is then read twice at most: with its script, and when
it is first called. (One inside a function that a dropped function had to
keep, such as a constructor, has no record of its own and is read again
when the dropped function is compiled.) The bracket lookahead skips them
too (`js_lazy_peek`), so the record notes whether the function had an `=`
token, the one thing the lookahead reports from inside a function.

**What is compiled with its parent.** Constructors and class static
blocks (they run when the class is defined or used); functions in a
`with` statement; a function written in parentheses or after `!`, which is
taken to be called at once (as V8 does), unless the function around it is
not compiled now either; and a function that calls `eval` directly, or
has one inside it that does, since eval can reach any variable around it.

**Closures.** A record keeps the names the function and those inside it
use and do not define (`js_lazy_collect`, the local half of
`resolve_scope_var`), private names apart. When the parent is compiled,
each is looked up from where the function was defined, as
`resolve_scope_var` would for a name it does not find locally, which marks
the variables it reaches as captured and gives the stub its closure
(`js_lazy_resolve_name`). An undefined private name is still a syntax
error of the script. Compiling the function later then works like direct
eval does: the function is a top level (`is_lazy_root`) whose names
outside it are looked up in that fixed closure, which stays in the order
of the walk so that a `with` object or a sloppy eval's variable object on
the way is tested only by the names found beyond it. The setter of a
private accessor has a name of its own, which is captured with it.

What a function takes from its definition site and the parser would have
read off the parent is kept: strictness before its own directives (a
class is strict code inside a sloppy function), the parent's kind (for
`yield` and `await` in arrow parameters), whether the code is a module.
Line and column numbers come out the same because the tokenizer is
restarted where it was (`JSLazyRecord.start`, `start_eol`, `start_mark`),
and `Function.prototype.toString`, `length` and `name` read what the stub
holds.

Measured on Linux (arm64, glibc profiling build), on a machine shared
with other work, the two ways from one build (`DASHER_EAGER_JS`, see
`crates/bench/README.md`):

- **Parse-only corpus** (1,296 distinct scripts from 18 recorded pages,
  128.4 MB, compiled and not run): 15.0 to 20.9 MB/s (8.55 to 6.15 s),
  and the compiled scripts hold 56% less of the JS heap (580 to 254 MiB).
- **16 heavy recorded pages** (CPU time and peak RSS, median of three
  runs, the two builds alternating page by page): 8% less peak RSS summed
  over the pages (1338 to 1230 MiB; 22% less on edition.cnn.com, 16% on
  espn.com, 13% on github.com, 11% on yahoo.com), and 3% more CPU time
  (41.0 to 42.3 s), with 7 to 9% more on theverge.com, wired.com and
  zillow.com. Element counts, tag counts, link counts and page text
  (digits masked) were the same on every page.

The CPU time goes two ways. Less is spent compiling, since the later
passes run only for the functions that are called, but a function that
is called is parsed twice, and parsing is two thirds of compiling. And
the smaller heap is collected more often, because a collection is due
when the heap has grown by half since the last one. On theverge.com,
6.5% more cycles over all: 0.25 G cycles more in the cycle collector, 0.4
G more parsing and scanning names, 0.17 G fewer in `resolve_variables`,
`resolve_labels` and `js_create_function`. A faster parser makes the
second of these smaller.

Tried and left out: taking a function written as an argument of a call
(`f(function () {...})`, the factory of a UMD wrapper) to be called at
once, as a function in parentheses is. Within noise on the 16 pages
(0.5% less CPU, 0.9% more memory).

`crates/js/tests/runtime.rs` checks closures, `with`, direct eval,
`arguments`, default parameters, classes and private names, `super`,
`new.target`, strict mode, generators, async functions, function
properties and text, stack positions and early errors against what the
engine gives with every function compiled with its script. test262 (tc39
`main` of 2026-11-13, 41,841 tests) fails the same tests either way.

## `quickjs/quickjs.c`: no dump code

Upstream turns its dump code on (`ENABLE_DUMPS`) whenever `NDEBUG` is
unset, and dasher's build never sets `NDEBUG`, to keep the assertions.
With it every string carries a 16-byte link in a list of all strings
(`JSRuntime.string_list`, read only to report leaks at exit), kept up to
date on every string allocation and free. Nothing in dasher or rquickjs
sets a dump flag. Here `ENABLE_DUMPS` is no longer defined by default; the
assertions stay. Build with `-DENABLE_DUMPS` to get the dumps back.

Measured: on 11 heavy recorded pages (Linux, aarch64, three runs each,
against the same build with the dump code), peak RSS 1 to 3 MiB less on
most (wired.com 176 to 173 MiB, techcrunch.com 90 to 88, angular.dev 43
to 40), none higher beyond run-to-run noise (theverge.com swings between
149 and 163 MiB in every build), and CPU time 0 to 4% less.

A smaller heap moves the points at which the cycle collector runs (when
the heap has grown by half since the last run), and a loop that keeps a
large structure alive can get one collection more or fewer. Before
functions were compiled lazily, "promise chain" (100,000 pending `then`s)
got one more: 1,240 to 1,330 ns per step, all of it in the collector, and
the same loop in a standalone `qjs` was unchanged. On current main it is
1,370 before and 1,346 after.

## `quickjs/quickjs.c`: `s += x` appends in place

QuickJS-NG appends to the left string in place when it holds the only
reference and its block has room (`JS_ConcatString`). In `s += x` on a
variable that never happens: the variable and the operand on the stack
both refer to the string, so each step copies the whole string into a new
one. Only `+=` with a constant, an argument or a variable on the right is
compiled to `OP_add_loc`, which reads the variable directly, and that one
duplicated the value before calling `JS_ConcatString` too.

Here the string path of `OP_add` (`js_add_string_in_place`) looks at the
instruction after it. When the sum is stored straight back
(`put_loc`/`set_loc`/`put_loc_check`/`put_var_ref`... with an optional
`dup`) into the variable the left operand was read from, and that string
has exactly those two references, no one can see the old string again, so
the right operand is appended in place. When the block is full the string
is grown with `js_realloc`, which keeps the slab allocator's rounding as
spare room; past 8 KiB an eighth is added, so long strings built in a
loop are not copied on every step. `s += a + b`, with `a` a string (an
`OP_add` directly followed by another), appends `a` and `b` without making
`a + b` first; concatenation of strings is associative, and the
intermediate string is never visible. `OP_add_loc` (`+=` with a
constant, argument or variable on the right) does the same when the
variable holds the only reference to its string. A string that is an atom
or a slice is never written to; upstream's in-place path now checks that
too.

`JS_ConcatString` also joins a string and an int32 without making a
string of the number first (`js_concat_string_int`).

Measured: the "string concat" micro-benchmark (`s += "ab" + (i & 7)`,
reset at 1,000 characters) goes from 159 to 78 ns per step; V8 without its
JIT takes 46; `s += "<li>"` from 129 to 66 ns. The rest is the
interpreter around the additions.
`crates/js/tests/fixtures/strings.js` checks aliases, closures, property
keys (atoms), wide strings, `s += s + x`, objects with `toString` and long
strings against what the engine gives without the change.

## `quickjs/quickjs.c`: JSON.parse reads the string itself

Upstream's `JSON.parse` converts its argument to UTF-8 and runs a token
reader over that (`JS_ParseJSON`): every string is built through a
`StringBuffer`, every key becomes a string and then an atom, every
property is defined through `JS_DefinePropertyValue` and every array
element appended one at a time, and every number goes through `strtod`
and comes out as a double.

`json_parse_fast` reads the 8-bit or 16-bit string directly. Values and
keys are collected on a value stack; an array is made with
`JS_NewArrayFrom` at its final length, and an object is made in one go
when its members are known. The context keeps the shape the last object
starting with each of 64 first atoms ended with (`ctx->json_shapes`): an
object whose keys come in the same order is made from that shape with
`JS_NewObjectFromShape`, and its keys are compared with the shape's atoms
instead of being looked up. Other keys are found with `__JS_FindAtom` on
the characters in place. A string without escapes is copied once.
Integers of up to 15 digits become int32 values where they fit (`-0`
stays a double); other numbers still go through `strtod` on the same
text. The object an object follows is held while its members are parsed,
since a nested object can replace it in the cache.

The fast parser only accepts valid JSON. On anything else, or when the
native stack runs short, it gives up and `JS_ParseJSON` runs on the same
text, so error messages and their positions are upstream's, and so is the
nesting depth at which the stack overflows (between 10,000 and 20,000
levels in dasher, either way). Property order, `__proto__` as an own key,
repeated keys (the last value wins, in the first position), lone
surrogates and revivers are as before: the reviver runs over the finished
value, as upstream's does.

Measured: parsing a 2.1 MB array of 20,000 records takes 10 ms instead of
38 (Node `--jitless`: 62), and the run's peak RSS falls from 27.0 to
23.9 MiB, since arrays are made at their length instead of grown.

## `quickjs/quickjs.c`: JSON.stringify writes plain objects from their shape

Upstream's `JSON.stringify` makes, for each object, an array of its keys
as strings with `Object.keys`, looks each key up again by string, quotes
it into a new string, and for each array element makes a string of its
index whether or not anything needs it. It checks for cycles with
`Array.prototype.includes` on an array it pushes and pops.

Here (`js_json_to_str`): a plain object whose shape is hashed, with no
key that sorts first as an array index and only data properties among
its enumerable string keys, is written straight from its shape. The
shape is held while its members are written, so it cannot change; if the
object gets another shape meanwhile (a `toJSON` or getter further down
changed it), the remaining keys are read with an ordinary `Get`, as the
spec has it. Fast array elements are read in place. Keys passed to
`toJSON` and replacers are made only when one is called. Strings are
quoted, and numbers formatted, straight into the output buffer. The cycle
check is a scan of a C array of the objects being written, and the
wrapper object `{"": value}` is made only for a replacer function. The
last shape found to qualify is remembered for the next object. Anything
else (replacer arrays, proxies, class instances with accessors, objects
with integer keys) takes upstream's path.

Measured: writing the 2.1 MB array above takes 12 ms instead of 53 (Node
`--jitless`: 16). The "JSON" micro-benchmark (`JSON.parse(JSON.stringify(o))`
of a small nested object) goes from 4,533 to 1,489 ns, with both changes;
V8 without its JIT takes 1,276.

`crates/js/tests/fixtures/json.js` checks key order, `__proto__`, repeated
keys, numbers, lone surrogates, error messages and their positions,
revivers, `toJSON` on objects, prototypes and BigInts, getters, objects
changed while they are written, replacer functions and arrays, gaps,
cycles, proxies and wrappers, plus 3,000 random documents (a third of them
broken) through parse and stringify, against what the engine gives without
the changes. V8 differs only in the wording of error messages.

## `quickjs/quickjs.c`: string search, split, replace and case

`indexOf`, `lastIndexOf`, `includes`, `startsWith` and `endsWith` compared
the strings character by character at every position (`string_cmp`). Here
8-bit strings are searched with `memchr` for the first character and
`memcmp` for the rest, and the others use `memcmp` where both strings are
the same width (`string_indexof`, `string_lastindexof`,
`string_match_at`). `replace` and `split` use the same search. `split`
collects its pieces and makes the array once, `replace` with a
replacement that has no `$` uses it as it is instead of running
`GetSubstitution`, and its buffer is made at the first match at about the
size of the result. `toLowerCase` and `toUpperCase` map an 8-bit string
within Latin-1 by table (except `toUpperCase` of µ, ß and ÿ, whose upper
case is outside it), and return the same string when nothing changes.
`JS_ToString` of a string returns it without a call.

Measured: the "string methods" micro-benchmark goes from 1,557 to 914 ns
per step; V8 without its JIT takes 581. The fixture in
`crates/js/tests/fixtures/strings.js` runs 3,000 random searches, splits,
replacements and case mappings over 8-bit, Latin-1 and wide strings with
surrogates and compares a hash of the answers with the engine's without
the change; V8 gives the same answers.

## `quickjs/quickjs.c`: an atom's index is kept in its string

`js_get_atom_index`, which every `obj[key]` with a string key that is
already an atom goes through, found an atom's index by walking its hash
chain. A string's `first_weak_ref` field is only ever used by symbols (a
symbol can be held weakly; nothing else in the atom table can), so for
other atoms it now holds the atom's index (`JSString.atom_index`, a union
with `first_weak_ref`), and `JS_FreeAtomStruct` only looks for weak
references on symbols.

Measured: 10 million `o[k]` reads with keys from `Object.keys`, 75 to
69 ns each.

## What the six changes above measure together

On 14 heavy recorded pages (Linux, aarch64 profiling build; main at
`045dfd2` with and without these changes, seven runs per page alternating
the builds, medians; the comparison was run twice): 2.9 and 3.0% less CPU
time summed over the pages (35.4 to 34.4 s, and 35.1 to 34.1), less on
every page in both runs but one where they tied (0.9 to 5.8%), and 1.3 and
1.1% less peak RSS (1135 to 1120 MiB, and 1132 to 1119), with no page
higher in both runs (nytimes.com was 1.9% lower in one and 1.5% higher in
the other). yahoo.com is left out: on this main it runs into a limit after
10 s in both builds. In profiles of ten of these
pages, the functions these changes replace or skip (`js_get_atom_index`,
`JS_ToQuotedString`, `JS_ToStringInternal`, `string_getc`,
`json_next_token`) went from 1.8% of the samples to 0.3%, `js_json_quote`
included; JSON and the String methods are now under 0.2% of CPU each
there. On the 2.1 MB JSON document above, parse and
stringify are 3.8 and 4.4 times faster.

The micro-benchmarks, against Node 22 `--jitless --single-threaded` on the
same machine (ns per operation, median of five runs):

| benchmark | before | after | V8 without JIT | after ÷ V8 |
| --- | ---: | ---: | ---: | ---: |
| string concat | 160 | 77.3 | 44.3 | 1.7× |
| string methods | 1,632 | 928 | 561 | 1.7× |
| JSON | 4,538 | 1,487 | 1,174 | 1.3× |

Most of what is left of the first two is the interpreter around the
calls (`let` loop counters going through `post_inc`'s slow path, method
lookups on a string) and allocation, which DAS-36 and DAS-46 work on.
With both of those merged in as they stood on 2026-11-14 the three come
to 55.2, 730 and 1,248 ns, 1.25, 1.3 and 1.06 times V8.

Tried and left out: a smaller load factor or a per-runtime cache in front
of the atom hash table. `__JS_FindAtom` is 2.5% of CPU on the heavy pages,
nearly all of it the parser looking up identifiers, but chains average
1.25 entries (measured over a megabyte of bundle source) and the time is
the cache misses on the bucket and the atom itself, which a different
table would not avoid. The conversions between UTF-8 and engine strings
at the Rust boundary (`JS_NewStringLen`, `JS_ToCStringLen2` and rquickjs's
`String::from_str`/`to_string`) came to under 0.5% of CPU on the heavy
pages, so they were left alone here.
