//! Compiles a corpus of scripts without running them and reports how fast
//! the engine got through them: the parse-only half of the real-page
//! benchmark (see `crates/bench/README.md`).
//!
//! ```sh
//! cargo run --profile profiling -p dasher-js --example compile_corpus -- DIR...
//! ```
//!
//! Every file under each `DIR` (or each file named) is compiled as a classic
//! script in a page runtime, or as a module if that fails, and freed. The
//! report gives the bytes compiled, the CPU time spent compiling, the rate
//! in MB/s, the most the JS heap grew by compiling one script,
//! and the scripts that compiled as neither. `--repeat N` compiles the
//! corpus N times and reports the fastest pass.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dasher_js::{JsRuntime, RuntimeOptions};
use rquickjs::qjs;

fn main() {
    let mut paths = Vec::new();
    let mut repeat = 1;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repeat" => {
                repeat = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--repeat N")
            }
            _ => collect(Path::new(&arg), &mut paths),
        }
    }
    paths.sort();
    let scripts: Vec<(PathBuf, CString)> = paths
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            // A script with a NUL in it is not text: leave it out.
            Some((path, CString::new(bytes).ok()?))
        })
        .collect();
    let bytes: usize = scripts.iter().map(|(_, s)| s.as_bytes().len()).sum();

    let mut best = Duration::MAX;
    let mut failed = Vec::new();
    let mut peak_heap = 0;
    let mut total_heap = 0;
    for _ in 0..repeat {
        let js = JsRuntime::new(RuntimeOptions::default()).expect("runtime");
        failed.clear();
        total_heap = 0;
        let mut total = Duration::ZERO;
        js.with(|ctx| {
            let raw = ctx.as_raw().as_ptr();
            // SAFETY: `raw` is live and the loader only makes empty modules.
            unsafe {
                qjs::JS_SetModuleLoaderFunc(
                    qjs::JS_GetRuntime(raw),
                    None,
                    Some(empty_module),
                    std::ptr::null_mut(),
                );
            }
            for (path, source) in &scripts {
                let name = CString::new(path.to_string_lossy().as_bytes()).unwrap();
                let (ok, took, heap) = compile(raw, source, &name);
                total += took;
                peak_heap = peak_heap.max(heap);
                total_heap += heap;
                if !ok {
                    failed.push(path.clone());
                }
            }
        });
        best = best.min(total);
    }
    let secs = best.as_secs_f64();
    println!(
        "{} scripts, {:.1} MB: {:.3} s, {:.1} MB/s; heap held by the compiled scripts: {:.1} MiB in all, {:.1} MiB at most for one",
        scripts.len(),
        bytes as f64 / 1e6,
        secs,
        bytes as f64 / 1e6 / secs,
        total_heap as f64 / (1024.0 * 1024.0),
        peak_heap as f64 / (1024.0 * 1024.0),
    );
    for path in &failed {
        println!("failed: {}", path.display());
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)
            .expect("readable directory")
            .flatten()
        {
            collect(&entry.path(), out);
        }
    } else {
        out.push(path.to_owned());
    }
}

/// Compiles `source` as a script, or as a module if that fails, and frees
/// the result. Returns whether either compiled, the CPU time the successful
/// compile took and how much the JS heap grew by it.
fn compile(ctx: *mut qjs::JSContext, source: &CString, name: &CString) -> (bool, Duration, i64) {
    let len = source.as_bytes().len() as _;
    for kind in [qjs::JS_EVAL_TYPE_GLOBAL, qjs::JS_EVAL_TYPE_MODULE] {
        let flags = (kind | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32;
        let before = heap_size(ctx);
        let start = cpu_time();
        // SAFETY: `ctx` is the live context of the runtime the caller holds
        // locked, and `source` and `name` are NUL-terminated and outlive the
        // call. The result, or the exception, is freed once.
        let (ok, took, grew) = unsafe {
            let value = qjs::JS_Eval(ctx, source.as_ptr(), len, name.as_ptr(), flags);
            let took = cpu_time() - start;
            let ok = !qjs::JS_IsException(value);
            let grew = heap_size(ctx) - before;
            if ok {
                qjs::JS_FreeValue(ctx, value);
            } else {
                qjs::JS_FreeValue(ctx, qjs::JS_GetException(ctx));
            }
            (ok, took, grew)
        };
        if ok {
            return (true, took, grew);
        }
    }
    (false, Duration::ZERO, 0)
}

/// Every import resolves to a module with no exports: compiling a module
/// loads what it imports, but nothing is linked or run.
unsafe extern "C" fn empty_module(
    ctx: *mut qjs::JSContext,
    name: *const std::ffi::c_char,
    _opaque: *mut std::ffi::c_void,
) -> *mut qjs::JSModuleDef {
    unsafe extern "C" fn init(_: *mut qjs::JSContext, _: *mut qjs::JSModuleDef) -> std::ffi::c_int {
        0
    }
    // SAFETY: called by the engine with a live context and a C string.
    unsafe { qjs::JS_NewCModule(ctx, name, Some(init)) }
}

fn heap_size(ctx: *mut qjs::JSContext) -> i64 {
    // SAFETY: `ctx` is live; the struct is plain data filled in by the call.
    unsafe {
        let mut usage: qjs::JSMemoryUsage = std::mem::zeroed();
        qjs::JS_ComputeMemoryUsage(qjs::JS_GetRuntime(ctx), &mut usage);
        usage.memory_used_size
    }
}

fn cpu_time() -> Duration {
    // SAFETY: `ts` is plain data written by the call.
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts);
        Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
    }
}
