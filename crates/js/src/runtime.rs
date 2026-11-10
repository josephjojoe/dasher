//! The per-page QuickJS runtime: limits, interrupts and error capture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rquickjs::context::EvalOptions;
use rquickjs::{
    Coerced, Context, Ctx, Exception, FromJs, Function, JsLifetime, Object, Runtime, Value, qjs,
};

use crate::clock::ClockMode;
use crate::console::{self, ConsoleMessage};
use crate::error::{ErrorKind, ScriptError};
use crate::event_loop::{self, DEFAULT_FRAME_INTERVAL, LoopHooks, LoopState, Stopped};
use crate::heap::{self, HeapState};
use crate::log::BoundedLog;
use crate::remote::Remote;

/// Default cap on the JS heap of one page.
pub const DEFAULT_MEMORY_LIMIT: usize = 256 * 1024 * 1024;
/// Default cap on the native stack JS execution may use.
pub const DEFAULT_MAX_STACK_SIZE: usize = 1024 * 1024;

const PRELUDE: &str = include_str!("prelude.js");

/// How a [`JsRuntime`] is set up.
#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    /// Upper bound on memory the JS heap may allocate, in bytes. Allocations
    /// past it fail with an out-of-memory error inside the script. `None`
    /// means unlimited.
    pub memory_limit: Option<usize>,
    /// Upper bound on native stack used by JS execution, in bytes. Deep
    /// recursion past it throws a `RangeError` instead of overflowing the host
    /// thread's stack, so keep it well below the stack of the thread that
    /// drives the runtime.
    pub max_stack_size: usize,
    /// Heap size, in bytes, at which the cycle collector first runs. `None`
    /// keeps the QuickJS default.
    pub gc_threshold: Option<usize>,
    /// How many script errors are kept before the oldest are dropped.
    pub error_capacity: usize,
    /// How many console messages are kept before the oldest are dropped.
    pub console_capacity: usize,
    /// Console messages longer than this many bytes are cut short.
    pub max_console_message_len: usize,
    /// How page time relates to the wall clock.
    pub clock: ClockMode,
    /// Spacing of the virtual frame clock that drives
    /// `requestAnimationFrame`.
    pub frame_interval: Duration,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            memory_limit: Some(DEFAULT_MEMORY_LIMIT),
            max_stack_size: DEFAULT_MAX_STACK_SIZE,
            gc_threshold: None,
            error_capacity: 1000,
            console_capacity: 1000,
            max_console_message_len: 16 * 1024,
            clock: ClockMode::default(),
            frame_interval: DEFAULT_FRAME_INTERVAL,
        }
    }
}

/// Stops JS running on a [`JsRuntime`] from another thread.
///
/// Once [`interrupt`](Self::interrupt) is called, running JS is aborted with
/// an uncatchable exception at its next interrupt check, and further JS on
/// that runtime is refused until [`JsRuntime::clear_interrupt`] is called.
#[derive(Clone)]
pub struct InterruptHandle {
    remote: Arc<Remote>,
}

impl std::fmt::Debug for InterruptHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterruptHandle").finish_non_exhaustive()
    }
}

impl InterruptHandle {
    /// Requests that the runtime stop running JS.
    pub fn interrupt(&self) {
        self.remote.interrupt();
    }
}

/// Host-side state shared between the runtime and the native functions it
/// exposes to JS. It holds no JS values, so it may outlive the context.
pub(crate) struct Shared {
    deadline: Cell<Option<Instant>>,
    pub(crate) remote: Arc<Remote>,
    /// Set when the interrupt handler stopped JS since the last `begin`.
    interrupted: Cell<bool>,
    heap: Rc<HeapState>,
    errors: RefCell<BoundedLog<ScriptError>>,
    pub(crate) console: RefCell<BoundedLog<ConsoleMessage>>,
    pub(crate) event_loop: LoopState,
}

impl Shared {
    fn new(options: &RuntimeOptions, heap: Rc<HeapState>) -> Self {
        Self {
            deadline: Cell::new(None),
            remote: Arc::default(),
            interrupted: Cell::new(false),
            heap,
            errors: RefCell::new(BoundedLog::new(options.error_capacity)),
            console: RefCell::new(BoundedLog::new(options.console_capacity)),
            event_loop: LoopState::new(options.clock, options.frame_interval),
        }
    }

    /// Why JS must not run right now, if it must not.
    pub(crate) fn stop_reason(&self) -> Option<Stopped> {
        if self.remote.kill.load(Ordering::Relaxed) {
            Some(Stopped::Interrupted)
        } else if self.deadline.get().is_some_and(|d| Instant::now() >= d) {
            Some(Stopped::DeadlineExceeded)
        } else {
            None
        }
    }

    fn should_interrupt(&self) -> bool {
        let stop = self.stop_reason().is_some();
        if stop {
            self.interrupted.set(true);
        }
        stop
    }

    /// Called before handing control to JS: refuses to start once the
    /// deadline has passed or an interrupt was requested.
    fn begin(&self) -> Result<(), ScriptError> {
        self.interrupted.set(false);
        self.heap.clear_refused();
        if self.should_interrupt() {
            return Err(ScriptError::interrupted());
        }
        Ok(())
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.interrupted.get()
    }

    pub(crate) fn report(&self, error: ScriptError) {
        self.errors.borrow_mut().push(error);
    }

    /// Turns an engine error into a [`ScriptError`], taking the pending
    /// exception off the context if there is one.
    pub(crate) fn capture<'js>(&self, ctx: &Ctx<'js>, error: rquickjs::Error) -> ScriptError {
        let mut captured = match error {
            rquickjs::Error::Exception => {
                let value = ctx.catch();
                // When QuickJS cannot even allocate the error object for an
                // allocation failure, it throws `null` instead.
                if value.is_null() && self.heap.take_refused() {
                    ScriptError::new(ErrorKind::OutOfMemory, "out of memory")
                } else {
                    error_from_value(ctx, value, ErrorKind::Uncaught)
                }
            }
            rquickjs::Error::Allocation => {
                ScriptError::new(ErrorKind::OutOfMemory, "out of memory")
            }
            other => ScriptError::new(ErrorKind::Internal, other.to_string()),
        };
        if self.interrupted.get() {
            captured.kind = ErrorKind::Interrupted;
        }
        captured
    }
}

/// JS values the host keeps for the lifetime of the context. Stored as
/// runtime userdata, which rquickjs drops before freeing the runtime.
pub(crate) struct JsData<'js> {
    inspect: Function<'js>,
    hooks: LoopHooks<'js>,
    /// Rejected promises that had no handler when they were rejected, with
    /// their reasons. Reported at the end of the next microtask checkpoint
    /// unless a handler is attached first.
    rejections: RefCell<Vec<(Value<'js>, Value<'js>)>>,
}

// SAFETY: `JsData` only holds values bound to `'js`, and `Changed` is the same
// struct with the lifetime substituted, so the layouts are identical.
unsafe impl<'js> JsLifetime<'js> for JsData<'js> {
    type Changed<'to> = JsData<'to>;
}

impl<'js> JsData<'js> {
    /// The event loop's JS hooks.
    pub(crate) fn hooks(ctx: &Ctx<'js>) -> rquickjs::Result<LoopHooks<'js>> {
        ctx.userdata::<JsData<'js>>()
            .map(|d| d.hooks.clone())
            .ok_or(rquickjs::Error::Unknown)
    }
}

/// Renders a JS value as text the way the console would.
pub(crate) fn inspect<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> String {
    let hook = ctx.userdata::<JsData<'js>>().map(|d| d.inspect.clone());
    if let Some(hook) = hook {
        match hook.call::<_, String>((value.clone(),)) {
            Ok(text) => return text,
            Err(_) => drop(ctx.catch()),
        }
    }
    match Coerced::<String>::from_js(ctx, value) {
        Ok(text) => text.0,
        Err(_) => {
            drop(ctx.catch());
            String::from("<unprintable value>")
        }
    }
}

/// Builds a [`ScriptError`] from a thrown or rejected JS value.
pub(crate) fn error_from_value<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
    kind: ErrorKind,
) -> ScriptError {
    let Some(exception) = value.as_object().cloned().and_then(Exception::from_object) else {
        return ScriptError::new(kind, inspect(ctx, value));
    };
    let name = exception
        .get::<_, Coerced<String>>("name")
        .map(|n| n.0)
        .unwrap_or_default();
    let message = exception.message().unwrap_or_default();
    let message = match (name.is_empty(), message.is_empty()) {
        (false, false) => format!("{name}: {message}"),
        (false, true) => name,
        (true, _) => message,
    };
    let kind = if message == "InternalError: out of memory" {
        ErrorKind::OutOfMemory
    } else {
        kind
    };
    let stack = exception.stack().filter(|s| !s.trim().is_empty());
    ScriptError {
        kind,
        message,
        stack,
    }
}

fn track_rejection<'js>(ctx: Ctx<'js>, promise: Value<'js>, reason: Value<'js>, handled: bool) {
    let Some(data) = ctx.userdata::<JsData<'js>>() else {
        return;
    };
    let mut pending = data.rejections.borrow_mut();
    if handled {
        pending.retain(|(p, _)| *p != promise);
    } else {
        pending.push((promise, reason));
    }
}

fn eval_options(filename: &str) -> EvalOptions {
    let mut options = EvalOptions::default();
    options.global = true;
    options.strict = false;
    options.filename = Some(filename.to_owned());
    options
}

/// A page's JavaScript engine.
///
/// Each page owns one: a QuickJS runtime and a single context with dasher's
/// globals installed. Dropping it frees every JS object the page created.
///
/// Script errors never panic or unwind into the host. They are captured into
/// an error log read with [`errors`](Self::errors) /
/// [`take_errors`](Self::take_errors), and returned where the call produces a
/// result.
///
/// A runaway script is stopped by setting a [`deadline`](Self::set_deadline)
/// or through an [`InterruptHandle`].
pub struct JsRuntime {
    context: Context,
    runtime: Runtime,
    shared: Rc<Shared>,
}

impl JsRuntime {
    /// Creates a runtime and context and installs the globals.
    pub fn new(options: RuntimeOptions) -> Result<Self, ScriptError> {
        let internal = |e: rquickjs::Error| ScriptError::new(ErrorKind::Internal, e.to_string());

        let (runtime, heap) = heap::new_runtime(options.memory_limit).map_err(internal)?;
        runtime.set_max_stack_size(options.max_stack_size);
        if let Some(threshold) = options.gc_threshold {
            runtime.set_gc_threshold(threshold);
        }

        let shared = Rc::new(Shared::new(&options, heap));
        let handler_shared = Rc::clone(&shared);
        runtime.set_interrupt_handler(Some(Box::new(move || handler_shared.should_interrupt())));
        runtime.set_host_promise_rejection_tracker(Some(Box::new(track_rejection)));

        let context = Context::full(&runtime).map_err(internal)?;
        let js = Self {
            context,
            runtime,
            shared,
        };
        let max_message_len = options.max_console_message_len;
        js.with(|ctx| {
            install(&ctx, &js.shared, max_message_len).map_err(|e| js.shared.capture(&ctx, e))
        })?;
        Ok(js)
    }

    pub(crate) fn shared(&self) -> &Rc<Shared> {
        &self.shared
    }

    /// Runs `f` with direct access to the engine context, for host bindings.
    ///
    /// No interrupt check or error capture happens here: the caller handles
    /// the `rquickjs` results itself. Microtasks queued by `f` run at the next
    /// checkpoint.
    pub fn with<R>(&self, f: impl for<'js> FnOnce(Ctx<'js>) -> R) -> R {
        self.context.with(|ctx| {
            // SAFETY: `ctx` is a live context and its runtime is locked for the
            // duration of this closure, on this thread. Moving the stack top to
            // the current stack pointer only changes where QuickJS measures its
            // stack limit from, so the limit holds wherever the host enters JS.
            unsafe { qjs::JS_UpdateStackTop(qjs::JS_GetRuntime(ctx.as_raw().as_ptr())) };
            f(ctx)
        })
    }

    /// Runs a classic script as one task, then performs a microtask
    /// checkpoint.
    ///
    /// `name` is used as the file name in stack traces. An exception the
    /// script throws is recorded in the error log and returned.
    pub fn run_script(&self, source: &str, name: &str) -> Result<(), ScriptError> {
        let result =
            self.run_task(|ctx| ctx.eval_with_options::<(), _>(source, eval_options(name)));
        self.perform_microtask_checkpoint();
        result
    }

    /// Evaluates `source` and returns its completion value serialized with
    /// `JSON.stringify`, or `None` when that yields `undefined` (for
    /// functions, symbols and `undefined` itself). Performs a microtask
    /// checkpoint afterwards.
    ///
    /// This is for the host pulling values out of the page: an exception is
    /// returned rather than recorded in the page's error log.
    pub fn eval_json(&self, source: &str) -> Result<Option<String>, ScriptError> {
        self.shared.begin()?;
        let result = self.with(|ctx| {
            let run = || -> rquickjs::Result<Option<String>> {
                let value: Value<'_> =
                    ctx.eval_with_options(source, eval_options("dasher:eval"))?;
                ctx.json_stringify(value)?
                    .map(|s| s.to_string())
                    .transpose()
            };
            run().map_err(|e| self.shared.capture(&ctx, e))
        });
        self.perform_microtask_checkpoint();
        result
    }

    /// Runs `f` as a task: refuses to start past the deadline, and records
    /// any error it raises.
    pub(crate) fn run_task(
        &self,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<()>,
    ) -> Result<(), ScriptError> {
        self.shared.begin()?;
        self.with(|ctx| f(&ctx).map_err(|e| self.shared.capture(&ctx, e)))
            .inspect_err(|e| self.shared.report(e.clone()))
    }

    /// Runs promise jobs and queued microtasks until none are left, then
    /// reports promises rejected without a handler.
    ///
    /// Stops early if the deadline passes or an interrupt is requested; the
    /// remaining jobs stay queued.
    pub fn perform_microtask_checkpoint(&self) {
        if self.shared.begin().is_err() {
            return;
        }
        // Moves the stack top to here; jobs run a few frames deeper.
        self.with(|_| ());
        while !self.shared.interrupted() {
            match self.runtime.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => break,
                Err(job) => job.0.with(|ctx| {
                    let error = self.shared.capture(&ctx, rquickjs::Error::Exception);
                    self.shared.report(error);
                }),
            }
        }
        self.with(|ctx| {
            let rejections = ctx
                .userdata::<JsData<'_>>()
                .map(|d| d.rejections.take())
                .unwrap_or_default();
            for (_promise, reason) in rejections {
                let error = error_from_value(&ctx, reason, ErrorKind::UnhandledRejection);
                self.shared.report(error);
            }
        });
    }

    /// Whether promise jobs or microtasks are waiting to run.
    pub fn has_pending_microtasks(&self) -> bool {
        self.runtime.is_job_pending()
    }

    /// Sets the wall-clock time after which running JS is interrupted and no
    /// more JS is started. `None` removes the deadline.
    pub fn set_deadline(&self, deadline: Option<Instant>) {
        self.shared.deadline.set(deadline);
    }

    /// The current deadline, if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.shared.deadline.get()
    }

    /// A handle that interrupts this runtime from any thread.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        InterruptHandle {
            remote: Arc::clone(&self.shared.remote),
        }
    }

    /// Withdraws a request made through an [`InterruptHandle`], allowing JS
    /// to run again.
    pub fn clear_interrupt(&self) {
        self.shared.remote.kill.store(false, Ordering::SeqCst);
    }

    /// The script errors recorded so far, oldest first.
    pub fn errors(&self) -> Vec<ScriptError> {
        self.shared.errors.borrow().iter().cloned().collect()
    }

    /// Removes and returns the recorded script errors, oldest first.
    pub fn take_errors(&self) -> Vec<ScriptError> {
        self.shared.errors.borrow_mut().take()
    }

    /// How many errors were discarded because the error log was full.
    pub fn dropped_errors(&self) -> u64 {
        self.shared.errors.borrow().dropped()
    }

    /// Bytes currently allocated by the JS heap.
    pub fn memory_allocated(&self) -> usize {
        usize::try_from(self.runtime.memory_usage().malloc_size).unwrap_or(0)
    }

    /// Runs the cycle collector now.
    pub fn run_gc(&self) {
        self.runtime.run_gc();
    }
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        // Queued host tasks may own JS values; release them while the
        // runtime is still alive.
        self.shared.remote.close();
        self.shared.event_loop.clear();
    }
}

/// Installs the host functions and evaluates the prelude.
fn install<'js>(
    ctx: &Ctx<'js>,
    shared: &Rc<Shared>,
    max_console_message_len: usize,
) -> rquickjs::Result<()> {
    let native = Object::new(ctx.clone())?;

    let s = Rc::clone(shared);
    native.set(
        "reportError",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, value: Value<'js>| {
            s.report(error_from_value(&ctx, value, ErrorKind::Uncaught));
        })?,
    )?;

    event_loop::install(ctx, &native, shared)?;
    console::install(ctx, &native, shared, max_console_message_len)?;

    let prelude: Function<'js> = ctx.eval_with_options(PRELUDE, eval_options("dasher:prelude"))?;
    let hooks: Object<'js> = prelude.call((native,))?;
    let data = JsData {
        inspect: hooks.get("inspect")?,
        hooks: LoopHooks::from_prelude(&hooks)?,
        rejections: RefCell::default(),
    };
    if ctx.store_userdata(data).is_err() {
        return Err(rquickjs::Error::Unknown);
    }
    Ok(())
}
