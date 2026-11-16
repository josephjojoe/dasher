//! Names the engine build, for the script cache's files on disk, and
//! compiles the glue.
//!
//! Bytecode is only good for the QuickJS it was written by. The cache's
//! keys and files carry `DASHER_ENGINE_ID`, a hash of the engine's sources
//! (`vendor/rquickjs-sys/quickjs`), this crate's version and the target,
//! so a cache directory that outlives a build is not read by the next one.
//!
//! The glue (`src/*.js`, see `src/glue.rs`) is compiled here to bytecode
//! that the crate includes, by the same engine built for the host. The
//! format does not depend on the target: it is little-endian everywhere
//! and names atoms by their text.

use std::ffi::{CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};

use rquickjs_sys as qjs;

fn main() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/rquickjs-sys/quickjs");
    println!("cargo:rerun-if-changed={}", engine.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut hash = Fnv::default();
    hash.write(env!("CARGO_PKG_VERSION").as_bytes());
    hash.write(std::env::var("TARGET").unwrap_or_default().as_bytes());
    let mut files: Vec<_> = fs::read_dir(&engine)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for file in files {
        if let Ok(bytes) = fs::read(&file) {
            hash.write(file.file_name().unwrap_or_default().as_encoded_bytes());
            hash.write(&bytes);
        }
    }
    println!(
        "cargo:rustc-env=DASHER_ENGINE_ID={:016x}{:016x}",
        hash.a, hash.b
    );

    compile_glue();
}

/// Compiles each `src/NAME.js` to `$OUT_DIR/glue/NAME.bc`: stripped
/// bytecode for a classic script called `dasher:NAME`, with `-` for `_`.
fn compile_glue() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // The whole directory, so that a new file is seen.
    println!("cargo:rerun-if-changed={}", src.display());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("glue");
    fs::create_dir_all(&out).expect("cannot create the glue directory");

    let mut scripts: Vec<_> = fs::read_dir(&src)
        .expect("cannot read src")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some("js".as_ref()))
        .collect();
    scripts.sort();

    for path in scripts {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("file name");
        let name = format!("dasher:{}", stem.replace('_', "-"));
        let source = fs::read(&path).expect("cannot read a glue script");
        let bytes =
            compile(&name, &source).unwrap_or_else(|e| panic!("compiling {}: {e}", path.display()));
        fs::write(out.join(format!("{stem}.bc")), bytes).expect("cannot write glue bytecode");
    }
}

/// Compiles a classic script to bytecode without source or debug
/// information, or returns the exception's message.
///
/// Each script gets a runtime of its own, because what a runtime writes
/// depends on what it compiled before (functions list their closure
/// variables in another order), and the bytecode should depend on the
/// script alone.
fn compile(name: &str, source: &[u8]) -> Result<Vec<u8>, String> {
    let source = CString::new(source).map_err(|e| e.to_string())?;
    let name = CString::new(name).map_err(|e| e.to_string())?;
    // SAFETY: the runtime and context are made here, used on this thread
    // only and freed before returning, context first. The C strings
    // outlive the calls that read them. The compiled function and the
    // written buffer are freed exactly once.
    unsafe {
        let runtime = qjs::JS_NewRuntime();
        assert!(!runtime.is_null(), "cannot create a JS runtime");
        let ctx = qjs::JS_NewContext(runtime);
        assert!(!ctx.is_null(), "cannot create a JS context");
        let function = qjs::JS_Eval(
            ctx,
            source.as_ptr(),
            source.as_bytes().len() as _,
            name.as_ptr(),
            (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
        );
        let result = if qjs::JS_IsException(function) {
            Err(exception(ctx))
        } else {
            let mut len = 0;
            let buf = qjs::JS_WriteObject(
                ctx,
                &mut len,
                function,
                (qjs::JS_WRITE_OBJ_BYTECODE
                    | qjs::JS_WRITE_OBJ_STRIP_SOURCE
                    | qjs::JS_WRITE_OBJ_STRIP_DEBUG) as i32,
            );
            qjs::JS_FreeValue(ctx, function);
            if buf.is_null() {
                Err(exception(ctx))
            } else {
                let bytes = std::slice::from_raw_parts(buf, len as usize).to_vec();
                qjs::js_free(ctx, buf.cast());
                Ok(bytes)
            }
        };
        qjs::JS_FreeContext(ctx);
        qjs::JS_FreeRuntime(runtime);
        result
    }
}

/// Takes the pending exception as text: its message, then its stack,
/// which has the line.
///
/// # Safety
///
/// `ctx` must be a live context that no other thread is using.
unsafe fn exception(ctx: *mut qjs::JSContext) -> String {
    // SAFETY: `ctx` is live, by this function's contract. Each value and
    // C string is freed once, after its last use.
    unsafe {
        let error = qjs::JS_GetException(ctx);
        let stack = qjs::JS_GetPropertyStr(ctx, error, c"stack".as_ptr());
        let mut text = String::new();
        for value in [error, stack] {
            let chars = qjs::JS_ToCString(ctx, value);
            if !chars.is_null() {
                text.push_str(&CStr::from_ptr(chars).to_string_lossy());
                text.push('\n');
                qjs::JS_FreeCString(ctx, chars);
            }
            qjs::JS_FreeValue(ctx, value);
        }
        text
    }
}

/// Two FNV-1a hashes with different offsets: 128 bits, enough to tell
/// builds apart.
struct Fnv {
    a: u64,
    b: u64,
}

impl Default for Fnv {
    fn default() -> Self {
        Self {
            a: 0xcbf2_9ce4_8422_2325,
            b: 0x6c62_272e_07bb_0142,
        }
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes.iter().chain(&[0xff]) {
            self.a = (self.a ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            self.b = (self.b ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}
