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


## `quickjs/quickjs.c`, `quickjs.h`: `JS_SetMallocFunctions`

A new call, `JS_SetMallocFunctions(rt, mf, opaque)`, replaces the
allocator functions a runtime was created with. The new functions must
manage the same blocks as the old ones, since blocks allocated before the
call are freed, resized and measured through the new ones.

rquickjs creates a runtime with its own C functions, which reach the
allocator through a boxed `dyn Allocator`: three calls for each `malloc`
and `free` before dasher's slab heap gets the request. dasher-js
(`crates/js/src/heap.rs`) creates the runtime that way, then points
QuickJS at its own `extern "C"` functions over the same heap, so each
request is one call. Measured with the slab fast paths inlined into those
functions (`crates/js/src/slab.rs`): 2.4% fewer cycles on 12 recorded
heavy pages, and no change in memory.

## `quickjs/quickjs.c`: reference counts are tested inline

QuickJS-NG exports `JS_FreeValue`, `JS_FreeValueRT`, `JS_DupAtom`,
`JS_DupAtomRT`, `JS_FreeAtom` and `JS_FreeAtomRT`, and quickjs.c calls
them itself, about 2,000 times for values alone. The library is built
with `-fPIC`, so the compiler may not inline an exported function into
its callers, and every release of a value was a call, even of a number
or `undefined`. On the heavy pages `JS_FreeValue` and `JS_FreeValueRT`
were 4% of CPU time in their own code.

Here quickjs.c defines function-like macros with those names that expand
to inline versions (`js_free_value_inline`, `js_dup_atom_inline`,
`js_free_atom_inline`). The reference count is tested and decremented at
the caller, and only freeing a value is a call (`js_free_value_rt`, kept
out of line). The exported functions are defined with their names in
parentheses, so the macros do not touch them, and callers outside
quickjs.c, rquickjs among them, call them as before. Code in quickjs.c
that defines a function with one of these names has to do the same.

Measured on 12 recorded heavy pages (Linux, musl, five runs): 4% fewer
cycles and instructions, the same peak RSS. The cost is code size: 53 KB
more text in quickjs.o for the values and 10 KB for the atoms, and the
release binary is 70 KiB larger (5.69 to 5.76 MiB). Idle RSS rises by the
same 68 KiB, the text the process touches. Inlining only the tag test,
with the decrement out of line, saved 22 KB of the 53 but gave back
three quarters of the gain (2.8% more cycles than full inlining), so it
was left out.

## `quickjs/quickjs.c`: the cycle collector marks children directly

`mark_children` visits each child of an object through the
`JS_MarkFunc` pointer that each collector pass passes in, and
`JS_MarkValue` is exported, so each child was two calls. Arrays,
`arguments` objects and functions were marked through the class table's
function pointer as well.

Here `mark_children` is `force_inline`: each pass (`gc_decref`,
`gc_scan`) gets its own copy with its constant mark function, which the
compiler then inlines. `JS_MarkValue` gets an inline version like
`JS_FreeValue` above. Plain arrays, `arguments` objects and bytecode
functions, the most common objects with a class mark function, are marked
by inlined copies of `js_array_mark` and `js_bytecode_function_mark`
instead of through the class table. Every other class still goes through
the table.

The collector does the same work in the same order. On 12 recorded heavy
pages it is 1.2% fewer cycles over all, with no change in memory.
