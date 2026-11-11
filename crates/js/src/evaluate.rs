//! Host evaluation that waits for promises: [`JsRuntime::evaluate`].

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use rquickjs::function::This;
use rquickjs::{Ctx, Function, Promise, Value};

use crate::error::{ErrorKind, ScriptError};
use crate::runtime::{JsRuntime, Shared, error_from_value, eval_options};

/// A JSON result, or the error that stands in for it.
type Outcome = Result<Option<String>, ScriptError>;
type Slot = Rc<RefCell<Option<Outcome>>>;

/// What [`JsRuntime::evaluate`] produced.
#[derive(Debug)]
pub enum Evaluation {
    /// The value, serialized with `JSON.stringify`; `None` when that
    /// yields `undefined`.
    Ready(Option<String>),
    /// The value is a promise that has not settled yet. Run the event loop
    /// and [`poll`](PendingEvaluation::poll) it.
    Pending(PendingEvaluation),
}

/// A promise returned by [`JsRuntime::evaluate`] that has not settled yet.
///
/// The host holds no JS value: the promise's reactions write the outcome
/// here when it settles. Dropping this, or the runtime, simply discards
/// the outcome.
pub struct PendingEvaluation {
    slot: Slot,
}

impl fmt::Debug for PendingEvaluation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingEvaluation")
            .field("settled", &self.slot.borrow().is_some())
            .finish()
    }
}

impl PendingEvaluation {
    /// The promise's outcome once it has settled: its value serialized
    /// with `JSON.stringify`, or the rejection as an error. Returns it
    /// only once; `None` until then and afterwards.
    pub fn poll(&self) -> Option<Result<Option<String>, ScriptError>> {
        self.slot.borrow_mut().take()
    }
}

impl JsRuntime {
    /// Evaluates `source` for the host, like
    /// [`eval_json`](Self::eval_json), and also waits for asynchronous
    /// results:
    ///
    /// - when the completion value is a function, such as
    ///   `() => document.title` or `async () => …`, it is called with no
    ///   arguments and its return value is used;
    /// - when the value is then a promise, its fulfilment value is used
    ///   once it settles, and a rejection becomes the error. If it has not
    ///   settled after the microtask checkpoint that follows evaluation,
    ///   [`Evaluation::Pending`] is returned: run the event loop and poll
    ///   it.
    ///
    /// Values are serialized with `JSON.stringify`. Exceptions, rejections
    /// and values that cannot be serialized (cycles, `BigInt`) are returned
    /// as errors rather than recorded in the page's error log, and a
    /// rejection is not reported to the page as unhandled.
    pub fn evaluate(&self, source: &str) -> Result<Evaluation, ScriptError> {
        self.shared().begin()?;
        let slot: Slot = Rc::default();
        let started = self.with(|ctx| {
            let run = || -> rquickjs::Result<Option<Promise<'_>>> {
                let mut value: Value<'_> =
                    ctx.eval_with_options(source, eval_options("dasher:eval"))?;
                if let Some(function) = value.as_function() {
                    value = function.call(())?;
                }
                if let Some(promise) = value.as_promise() {
                    return Ok(Some(promise.clone()));
                }
                *slot.borrow_mut() = Some(stringify(&ctx, self.shared(), value));
                Ok(None)
            };
            let promise = run().map_err(|e| self.shared().capture(&ctx, e))?;
            if let Some(promise) = promise {
                settle_into(&ctx, self.shared(), promise, &slot)
                    .map_err(|e| self.shared().capture(&ctx, e))?;
            }
            Ok::<_, ScriptError>(())
        });
        self.perform_microtask_checkpoint();
        self.shared().check_limits(started)?;
        let pending = PendingEvaluation { slot };
        match pending.poll() {
            Some(outcome) => outcome.map(Evaluation::Ready),
            None => Ok(Evaluation::Pending(pending)),
        }
    }
}

/// `JSON.stringify(value)`, with a throw turned into an error.
fn stringify<'js>(ctx: &Ctx<'js>, shared: &Shared, value: Value<'js>) -> Outcome {
    ctx.json_stringify(value)
        .map(|s| s.map(|s| s.to_string()).transpose())
        .and_then(|s| s)
        .map_err(|e| shared.capture(ctx, e))
}

/// Attaches reactions to `promise` that write its outcome into `slot`.
/// Handling the rejection keeps it from being reported as unhandled.
fn settle_into<'js>(
    ctx: &Ctx<'js>,
    shared: &Rc<Shared>,
    promise: Promise<'js>,
    slot: &Slot,
) -> rquickjs::Result<()> {
    let (s, fulfilled) = (Rc::clone(shared), Rc::clone(slot));
    let on_fulfilled = Function::new(ctx.clone(), move |ctx: Ctx<'js>, value: Value<'js>| {
        *fulfilled.borrow_mut() = Some(stringify(&ctx, &s, value));
    })?;
    let rejected = Rc::clone(slot);
    let on_rejected = Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
        let error = error_from_value(&ctx, reason, ErrorKind::Uncaught);
        *rejected.borrow_mut() = Some(Err(error));
    })?;
    let then: Function<'js> = promise.get("then")?;
    then.call::<_, Value<'js>>((This(promise), on_fulfilled, on_rejected))?;
    Ok(())
}
