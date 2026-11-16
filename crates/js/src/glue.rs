//! The JS glue evaluated into every new context, kept as precompiled
//! bytecode.
//!
//! Compiling the glue from source cost a process more CPU time than the
//! rest of loading a small page, and the resulting functions keep their
//! source text and line tables alive in every page's heap. Instead, the
//! build script compiles the glue to bytecode without source or debug
//! information, the bytecode is part of the binary, and each new context
//! only loads it. As a side effect, glue functions print as
//! `[native code]` from `Function.prototype.toString`, like browser
//! built-ins.

use rquickjs::{Ctx, Exception, Value, qjs};

use crate::error::ScriptError;

/// A glue script: its name, and its bytecode as `build.rs` wrote it.
struct Glue {
    name: &'static str,
    code: &'static [u8],
    /// What `code` was compiled from.
    #[cfg(test)]
    source: &'static str,
}

/// The script `src/FILE.js`, which `build.rs` compiled as `NAME`.
macro_rules! glue {
    ($name:literal, $file:literal) => {
        Glue {
            name: $name,
            code: include_bytes!(concat!(env!("OUT_DIR"), "/glue/", $file, ".bc")),
            #[cfg(test)]
            source: include_str!(concat!($file, ".js")),
        }
    };
}

/// The prelude, evaluated first. It takes `native` and returns the shared
/// `internals` object.
const PRELUDE: Glue = glue!("dasher:prelude", "prelude");

/// Glue scripts evaluated after the prelude, in order. Each evaluates to a
/// function taking `native` and the shared `internals` object, and
/// installs its globals.
const GLUE: &[Glue] = &[
    glue!("dasher:events", "events"),
    glue!("dasher:encoding", "encoding"),
    glue!("dasher:url", "url"),
    glue!("dasher:clone", "clone"),
    glue!("dasher:crypto", "crypto"),
    glue!("dasher:window", "window"),
    glue!("dasher:storage", "storage"),
    glue!("dasher:media", "media"),
    glue!("dasher:lazy", "lazy"),
    // Every stub is in this one file, which lists them all.
    glue!("dasher:stubs", "stubs"),
    glue!("dasher:fetch", "fetch"),
    glue!("dasher:xhr", "xhr"),
    glue!("dasher:dom", "dom"),
    glue!("dasher:mutation", "mutation"),
    glue!("dasher:custom-elements", "custom_elements"),
    glue!("dasher:intersection", "intersection"),
    glue!("dasher:loader", "loader"),
];

/// Glue evaluated only when `lazy.js` asks for it through
/// `native.loadGlue(name)`. Each evaluates to a function taking `native`
/// and `internals` and returning what it implements; `lazy.js` installs
/// that.
const LAZY: &[Glue] = &[
    glue!("dasher:intl", "intl"),
    glue!("dasher:streams", "streams"),
];

/// Does nothing: the glue is compiled when dasher is built.
///
/// In 0.1.0 the first [`JsRuntime`](crate::JsRuntime) in a process
/// compiled the glue, and this did that ahead of the first page load.
/// It is kept so that code calling it still builds.
pub fn precompile() -> Result<(), ScriptError> {
    Ok(())
}

/// Runs precompiled bytecode in `ctx` and returns its completion value.
fn run<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> rquickjs::Result<Value<'js>> {
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `raw` is the live context `ctx` wraps. The bytecode was
    // written by `compile` with the same QuickJS build. `JS_EvalFunction`
    // takes ownership of the function it is given, and the value it returns
    // is handed to `Value::from_raw`, which owns it from then on.
    unsafe {
        let function = qjs::JS_ReadObject(
            raw,
            bytes.as_ptr(),
            bytes.len() as _,
            qjs::JS_READ_OBJ_BYTECODE as i32,
        );
        if qjs::JS_IsException(function) {
            return Err(rquickjs::Error::Exception);
        }
        let value = qjs::JS_EvalFunction(raw, function);
        if qjs::JS_IsException(value) {
            return Err(rquickjs::Error::Exception);
        }
        Ok(Value::from_raw(ctx.clone(), value))
    }
}

/// Evaluates the prelude and every glue script, returning each completion
/// value (the functions the glue files evaluate to) in order.
pub(crate) fn load<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Vec<Value<'js>>> {
    std::iter::once(&PRELUDE)
        .chain(GLUE)
        .map(|glue| run(ctx, glue.code))
        .collect()
}

/// Evaluates the [`LAZY`] glue script called `name` and returns its
/// completion value.
pub(crate) fn load_lazy<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Value<'js>> {
    let glue = LAZY
        .iter()
        .find(|glue| glue.name == name)
        .ok_or_else(|| Exception::throw_internal(ctx, &format!("no lazy glue {name}")))?;
    run(ctx, glue.code)
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use rquickjs::{Context, Runtime};

    use super::*;

    /// Compiles a classic script to stripped bytecode, as `build.rs` does.
    fn compile(ctx: &Ctx<'_>, name: &str, source: &str) -> Vec<u8> {
        let source = CString::new(source).unwrap();
        let name = CString::new(name).unwrap();
        let raw = ctx.as_raw().as_ptr();
        // SAFETY: `raw` is the live context `ctx` wraps, locked for this
        // call. The C strings outlive the calls that read them. The
        // compiled function and the written buffer are freed exactly once.
        unsafe {
            let function = qjs::JS_Eval(
                raw,
                source.as_ptr(),
                source.as_bytes().len() as _,
                name.as_ptr(),
                (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
            );
            assert!(!qjs::JS_IsException(function), "{:?}", ctx.catch());
            let mut len = 0;
            let buf = qjs::JS_WriteObject(
                raw,
                &mut len,
                function,
                (qjs::JS_WRITE_OBJ_BYTECODE
                    | qjs::JS_WRITE_OBJ_STRIP_SOURCE
                    | qjs::JS_WRITE_OBJ_STRIP_DEBUG) as i32,
            );
            qjs::JS_FreeValue(raw, function);
            assert!(!buf.is_null(), "{:?}", ctx.catch());
            let bytes = std::slice::from_raw_parts(buf, len as usize).to_vec();
            qjs::js_free(raw, buf.cast());
            bytes
        }
    }

    /// The bytecode in the binary was written by the build script with the
    /// engine built for the host. This engine, built for the target,
    /// writes the same from the same source.
    #[test]
    fn built_bytecode_is_this_engines() {
        for glue in std::iter::once(&PRELUDE).chain(GLUE).chain(LAZY) {
            // A runtime each, as in `build.rs`.
            let runtime = Runtime::new().unwrap();
            let context = Context::full(&runtime).unwrap();
            let compiled = context.with(|ctx| compile(&ctx, glue.name, glue.source));
            assert!(compiled == glue.code, "{} differs", glue.name);
        }
    }
}
