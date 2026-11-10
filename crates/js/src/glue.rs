//! The JS glue evaluated into every new context, kept as precompiled
//! bytecode.
//!
//! Compiling the glue from source costs several milliseconds, and the
//! resulting functions keep their source text and line tables alive in
//! every page's heap. Instead, the glue is compiled once per process in a
//! scratch runtime, written out as bytecode without source or debug
//! information, and each new context only loads that bytecode. As a side
//! effect, glue functions print as `[native code]` from
//! `Function.prototype.toString`, like browser built-ins.

use std::ffi::CString;
use std::sync::OnceLock;

use rquickjs::{Context, Ctx, Exception, Runtime, Value, qjs};

use crate::error::{ErrorKind, ScriptError};

/// The prelude, evaluated first. It takes `native` and returns the shared
/// `internals` object.
pub(crate) const PRELUDE: (&str, &str) = ("dasher:prelude", include_str!("prelude.js"));

/// Glue scripts evaluated after the prelude, in order, as `(name, source)`.
/// Each evaluates to a function taking `native` and the shared `internals`
/// object, and installs its globals.
pub(crate) const GLUE: &[(&str, &str)] = &[
    ("dasher:events", include_str!("events.js")),
    ("dasher:encoding", include_str!("encoding.js")),
    ("dasher:url", include_str!("url.js")),
    ("dasher:clone", include_str!("clone.js")),
    ("dasher:crypto", include_str!("crypto.js")),
    ("dasher:window", include_str!("window.js")),
    ("dasher:storage", include_str!("storage.js")),
];

/// Compiles the glue now rather than when the first runtime is created.
///
/// The first [`JsRuntime`](crate::JsRuntime) in a process otherwise pays
/// for compiling the glue (a few milliseconds). Call this during process
/// start-up, such as a serverless init phase, to take that off the first
/// page load. Later calls do nothing.
pub fn precompile() -> Result<(), ScriptError> {
    bytecode()
        .map(drop)
        .map_err(|e| ScriptError::new(ErrorKind::Internal, e))
}

/// Bytecode for the prelude followed by each entry of [`GLUE`].
fn bytecode() -> Result<&'static [Vec<u8>], String> {
    static BYTECODE: OnceLock<Result<Vec<Vec<u8>>, String>> = OnceLock::new();
    BYTECODE
        .get_or_init(compile_all)
        .as_deref()
        .map_err(Clone::clone)
}

fn compile_all() -> Result<Vec<Vec<u8>>, String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    context.with(|ctx| {
        std::iter::once(&PRELUDE)
            .chain(GLUE)
            .map(|&(name, source)| {
                compile(&ctx, name, source).map_err(|e| {
                    let detail = match e {
                        rquickjs::Error::Exception => format!("{:?}", ctx.catch()),
                        other => other.to_string(),
                    };
                    format!("compiling {name}: {detail}")
                })
            })
            .collect()
    })
}

/// Compiles a classic script to stripped bytecode.
fn compile(ctx: &Ctx<'_>, name: &str, source: &str) -> rquickjs::Result<Vec<u8>> {
    let source = CString::new(source)?;
    let name = CString::new(name)?;
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `raw` is the live context `ctx` wraps, locked for this call.
    // The C strings outlive the calls that read them. The compiled function
    // and the written buffer are freed exactly once, below.
    unsafe {
        let function = qjs::JS_Eval(
            raw,
            source.as_ptr(),
            source.as_bytes().len() as _,
            name.as_ptr(),
            (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
        );
        if qjs::JS_IsException(function) {
            return Err(rquickjs::Error::Exception);
        }
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
        if buf.is_null() {
            return Err(rquickjs::Error::Exception);
        }
        let bytes = std::slice::from_raw_parts(buf, len as usize).to_vec();
        qjs::js_free(raw, buf.cast());
        Ok(bytes)
    }
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
    let code = bytecode().map_err(|e| Exception::throw_internal(ctx, &e))?;
    code.iter().map(|bytes| run(ctx, bytes)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glue_compiles() {
        let code = bytecode().unwrap();
        assert_eq!(code.len(), GLUE.len() + 1);
        assert!(code.iter().all(|b| !b.is_empty()));
    }
}
