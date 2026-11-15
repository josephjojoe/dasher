#!/usr/bin/env python3
"""Split perf samples into kinds of work, as the audit of babeb38 did.

usage: classify.py dasher|lp STACKS.txt [--top N]

STACKS.txt is `perf script -F comm,tid,ip,sym,dso` output: for dasher one
sample per block (leaf first, DWARF call graphs), for Lightpanda one line
per sample (`-G`, leaf only: V8's frames do not unwind). Each sample goes to
the kind of its innermost recognizable frame. Copies (memcpy and the like)
take their caller's kind. Prints the share of samples per kind as JSON.
"""

import json
import re
import sys
from collections import Counter

KINDS = ["js", "compile", "dom", "gc", "regexp", "jit", "html", "net", "loop", "other"]

# dasher: Rust paths and QuickJS's C names.
PASS = re.compile(r"^(memcpy|memmove|memset|memcmp|bcmp|strlen|__mem|core::ptr::copy|"
                  r"<core::str|core::str::|core::slice::|alloc::raw_vec|alloc::vec|<alloc::vec|"
                  r"core::fmt|<alloc::string|alloc::string|alloc::fmt|__aarch64_|"
                  r"<std::collections::hash|hashbrown|<hashbrown|core::hash|<core::hash|"
                  r"std::thread::local|core::cell|<core::cell|core::ops::function|<core::ops)")
D_RULES = [
    ("regexp", re.compile(r"^(lre_|js_regexp|JS_RegExp|regexp_|js_reg|dasher_js::regexp|<dasher_js::regexp|"
                          r"js_string_.*replace|lre)|regex|RegExp")),
    ("gc", re.compile(r"^(gc_|mark_children|mark_|JS_RunGC|js_trigger_gc|free_object|free_function_bytecode|"
                      r"__JS_FreeValueRT|free_var_ref|js_free_shape|free_zero_refcount|reset_weak_ref|"
                      r"js_free_modules|free_gc_object|js_array_finalizer|js_c_function_data_finalizer|"
                      r"js_bytecode_function_finalizer|js_malloc|js_free|js_realloc|js_mallocz|js_def_|"
                      r"js_slab|dasher_js::slab|<dasher_js::slab|__libc_malloc|__libc_free|__libc_realloc|"
                      r"__libc_calloc|malloc|free|realloc|calloc|alloc_slot|enframe|get_meta|nontrivial_free|"
                      r"alloc_group|__malloc|__free|__rust_alloc|__rust_dealloc|__rust_realloc|"
                      r"<alloc::alloc|alloc::alloc|js_free_value_rt|JS_FreeValueRT|js_free_rt|js_malloc_rt)")),
    ("compile", re.compile(r"^(js_parse|next_token|simple_next_token|json_next_token|ident_realloc|"
                           r"js_create_function|resolve_variables|resolve_labels|compute_stack_size|"
                           r"skip_dead_code|add_var|add_arg|emit_|push_scope|pop_scope|js_compile|"
                           r"JS_ReadObject|JS_ReadFunction|bc_read|JS_ReadObjectAtoms|js_lazy|lazy_|cpool_|"
                           r"js_new_function_def|free_function_def|skip_|js_skip|optimize_scope|"
                           r"new_label|update_label|find_var|find_arg|code_match|instantiate_hoisted|"
                           r"convert_|get_prev_opcode|js_parse_template|re_parse|ss_|"
                           r"dasher_js::script_cache|<dasher_js::script_cache|zstd|ZSTD|HUF_|FSE_|"
                           r"js_scan|js_create_module|js_build_|closure_var|capture_scope|get_closure_var|"
                           r"js_emit|js_resolve|set_object_name|js_template|is_regexp_allowed|token_is|"
                           r"js_is_live_code|ident_|unicode_from|js_function_set_properties|"
                           r"JS_EvalFunction|js_dasher_lazy|compile_)")),
    ("html", re.compile(r"html5ever|markup5ever|tendril|dasher_dom::parse|<dasher_dom::parse|"
                        r"dasher_dom::stream|<dasher_dom::stream")),
    ("dom", re.compile(r"dasher_dom|dasher_css|dasher_js::dom|<dasher_js::dom|dasher_js::events|"
                       r"<dasher_js::events|dasher_js::host|selectors::|<selectors|cssparser|<cssparser|"
                       r"dasher_js::observer|dasher_js::mutation|<dasher_js::mutation|dasher_js::layout|"
                       r"dasher_js::style|<dasher_js::style|dasher_js::native|<dasher_js::native|"
                       r"dasher_js::wrapper|dasher_js::bind|<dasher_js::bind|dasher_js::loader|"
                       r"<dasher_js::loader")),
    ("net", re.compile(r"hyper|rustls|ring::|<ring|h2::|<h2|dasher_net|<dasher_net|tokio::net|"
                       r"http::|<http::|aes_|sha256|sha512|gcm_|chacha|poly1305|x25519|ecp_nistz|"
                       r"bn_|webpki|<webpki|flate2|miniz|brotli|<brotli|zlib|inflate|"
                       r"dasher_js::net|<dasher_js::net|dasher_js::fetch|<dasher_js::fetch")),
    ("loop", re.compile(r"dasher_page|<dasher_page|tokio|<tokio|dasher_js::event_loop|dasher_js::timers|"
                        r"<dasher_js::timers|dasher_js::runtime|<dasher_js::runtime|dasher_js::budget|"
                        r"dasher::|<dasher::|futures|<futures|epoll|clock_gettime|__clock_gettime|"
                        r"dasher_js::clock|<dasher_js::clock|serde_json|<serde_json|std::|<std::|"
                        r"dasher_js::remote|<dasher_js::remote")),
    ("js", re.compile(r"^(JS_|js_|__JS_|JS|qjs|rquickjs|<rquickjs|dasher_js|<dasher_js|"
                      r"js[A-Z]|string_|atom_|find_own_property|add_property|delete_property|"
                      r"build_|async_func|async_|__async|set_array_length|expand_fast_array|"
                      r"add_fast_array|convert_fast|get_|set_|free_property|shape_|resize_|"
                      r"JS_NewAtom|__JS_NewAtom|dbuf_|u32toa|i64toa|js_dtoa|dtoa|atod|"
                      r"unicode_|lre_is|is_|delete_|instantiate|perform_|call_|promise_|"
                      r"map_|js_map|stack_|free_|init_|new_|copy_|compare_|utf8|"
                      r"icu|<icu|dasher_js::intl|<dasher_js::intl|dasher_js::urls|<dasher_js::urls|"
                      r"url::|<url::|idna|<idna)")),
]

L_RULES = [
    ("regexp", re.compile(r"RegExp|regexp|Irregexp|irregexp")),
    ("jit", re.compile(r"compiler::|maglev::|baseline::|turboshaft::|Turbofan|turbofan|"
                       r"RegisterAllocator|InstructionSelector|LiveRange|CodeGenerator|Assembler|"
                       r"OptimizingCompile|TurbofanCompilationJob|JSHeapBroker|compiler::")),
    ("compile", re.compile(r"Parser|PreParser|Scanner|AstValueFactory|AstNodeFactory|Scope::|DeclarationScope|"
                           r"BytecodeGenerator|BytecodeArray|BytecodeRegister|Compiler::|CodeSerializer|"
                           r"UnoptimizedCompil|ParseInfo|FuncNameInferrer|ExpressionParsingScope|"
                           r"ConstantArrayBuilder|HandlerTableBuilder|SourcePositionTable|Zone::|"
                           r"AccountingAllocator|AstRawString|Builtins_CompileLazy|ExpressionScope|"
                           r"VariableProxy|interpreter::|Deserializer|ScriptCompiler|Rewriter|"
                           r"ParserBase|AstTraversal|SharedFunctionInfo::|compileWithCache|PreparseData|"
                           r"ScopeInfo|BackgroundCompile|LazyCompile")),
    ("gc", re.compile(r"Heap|heap::|Scavenge|MarkCompact|Marking|Sweep|ConcurrentMark|cppgc|"
                      r"MemoryAllocator|PagedSpace|NewSpace|SemiSpace|LocalHeap|MainAllocator|"
                      r"Evacuat|GCTracer|RecordWrite|WriteBarrier|MemoryChunk|PageMetadata|"
                      r"malloc|free|calloc|realloc|mmap|munmap|madvise|Allocate|Builtins_.*Allocat|"
                      r"YoungGeneration|MinorMS|RememberedSet|SlotSet|TypedSlot|StoreBuffer|"
                      r"Finaliz|WeakObjects|EphemeronHashTable|GlobalHandles|PersistentRegion")),
    ("html", re.compile(r"html5ever|markup5ever|tendril|parser\.html|Parser\.zig|browser\.parser")),
    ("net", re.compile(r"curl|Curl|ssl|SSL|bssl|EVP_|aes_|sha256|sha512|gcm_|chacha|poly1305|x25519|"
                       r"ecp_nistz|bn_|nghttp2|network\.|http\.|Http|BIO_|CRYPTO_|brotli|zlib|inflate|"
                       r"BrotliDecoder|pkey|X509|x509|ec_|p256|fiat_")),
    ("dom", re.compile(r"^(browser\.webapi|browser\.dom|browser\.css|browser\.html|browser\.js|webapi\.|"
                       r"js\.bridge|browser\.Page|Page\.|browser\.selector|selector\.|dom\.|css\.|"
                       r"v8::(?!internal)|browser\.Mutation|browser\.events|Event|Element|Node|"
                       r"browser\.EventManager|browser\.Document|browser\.Frame)")),
    ("loop", re.compile(r"Runner|ScriptManager|browser\.|xev|Loop|loop\.|App\.|lightpanda\.|"
                        r"clock_gettime|epoll|poll|pthread|futex|sched|Thread|std\.|mem\.|hash_map|"
                        r"array_list|heap\.|fmt\.|unicode\.|json\.|log\.")),
    ("js", re.compile(r"Builtins_|v8::internal|Runtime_|\[unknown\]|JIT|v8::base|std::__Cr|"
                      r"icu_|icu::|u_|ucol|ubrk")),
]


def classify_dasher(stack):
    for sym in stack:
        if PASS.match(sym):
            continue
        for kind, rx in D_RULES:
            if rx.search(sym):
                return kind
    return "other"


def classify_lp(sym, dso, ip=""):
    if "lightpanda" not in dso and "[unknown]" not in dso:
        # libc, the dynamic linker: allocation counts as memory, the rest as other.
        return "gc" if re.match(r"^(__libc_)?(malloc|free|calloc|realloc|cfree)|^_int_|^tcache|^arena", sym) else "other"
    if "[unknown]" in dso:
        # The vDSO (clock reads) sits at the top of the address space; other
        # unknown code is V8's generated code.
        return "loop" if ip.startswith("ffff") else "js"
    for kind, rx in L_RULES:
        if rx.search(sym):
            return kind
    return "js"


LINE = re.compile(r"^\s*([0-9a-f]+)\s+(.*?)\s+\((.*)\)\s*$")


def samples_dasher(path):
    stack = []
    for line in open(path, encoding="utf-8", errors="replace"):
        if not line.strip():
            if stack:
                yield stack
            stack = []
            continue
        m = LINE.match(line)
        if m:
            sym = re.sub(r"\+0x[0-9a-f]+$", "", m.group(2))
            stack.append(sym)
        elif not line.startswith(("\t", " ")):
            if stack:
                yield stack
            stack = []
    if stack:
        yield stack


def main():
    who, path = sys.argv[1], sys.argv[2]
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 0
    count = Counter()
    unk = Counter()
    if who == "dasher":
        for st in samples_dasher(path):
            k = classify_dasher(st)
            count[k] += 1
            if k in ("other",) and top:
                unk[next((s for s in st if not PASS.match(s)), st[0])] += 1
    else:
        rx = re.compile(r"^\s*\S+\s+\d+\s+([0-9a-f]+)\s+(.*?)\s+\((.*)\)\s*$")
        for line in open(path, encoding="utf-8", errors="replace"):
            m = rx.match(line)
            if not m:
                continue
            sym, dso = re.sub(r"\+0x[0-9a-f]+$", "", m.group(2)), m.group(3)
            k = classify_lp(sym, dso, m.group(1))
            count[k] += 1
            if k in ("other", "loop") and top:
                unk[sym] += 1
    total = sum(count.values())
    print(json.dumps({"samples": total, **{k: count[k] / total if total else 0 for k in KINDS}}))
    for s, n in unk.most_common(top):
        print(f"{n:6} {s}", file=sys.stderr)


if __name__ == "__main__":
    main()
