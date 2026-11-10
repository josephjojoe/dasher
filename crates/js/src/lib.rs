//! JavaScript for dasher.
//!
//! The QuickJS runtime (via rquickjs), the event loop (tasks, microtasks and
//! timers) and the bindings that expose the DOM and browser globals to page
//! scripts. Layout-dependent APIs return plausible synthetic values so that
//! content gated on visibility or scrolling still loads.

mod clock;
mod console;
mod error;
mod event_loop;
mod heap;
mod log;
mod remote;
mod run;
mod runtime;
mod timers;

pub use clock::ClockMode;
pub use console::{ConsoleLevel, ConsoleMessage};
pub use error::{ErrorKind, ScriptError};
pub use event_loop::{DEFAULT_FRAME_INTERVAL, LoopHandle, Stopped, Task};
pub use remote::{PendingOperation, SendTask, TaskSender};
pub use run::{RunLimits, RunOutcome};
pub use runtime::{
    DEFAULT_MAX_STACK_SIZE, DEFAULT_MEMORY_LIMIT, InterruptHandle, JsRuntime, RuntimeOptions,
};

/// The rquickjs version dasher is built against, for host bindings.
pub use rquickjs;
