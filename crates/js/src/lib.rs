//! JavaScript for dasher.
//!
//! The QuickJS runtime (via rquickjs), the event loop (tasks, microtasks and
//! timers) and the bindings that expose the DOM and browser globals to page
//! scripts. Layout-dependent APIs return plausible synthetic values so that
//! content gated on visibility or scrolling still loads.
//!
//! # Runtime and event loop
//!
//! Each page gets its own [`JsRuntime`]: one QuickJS runtime and context,
//! with a memory limit, a stack limit and an interrupt handler that stops
//! runaway scripts at a [deadline](JsRuntime::set_deadline) or on request
//! through an [`InterruptHandle`]. Exceptions, unhandled promise rejections
//! and out-of-memory conditions are captured as [`ScriptError`]s, never
//! propagated into the host.
//!
//! The event loop runs tasks (due timers and host tasks) one at a time, each
//! followed by a microtask checkpoint, and animation frame callbacks on a
//! virtual frame clock. It can be stepped synchronously with
//! [`run_ready`](JsRuntime::run_ready) and [`advance`](JsRuntime::advance),
//! or driven with [`run`](JsRuntime::run) under Tokio, which waits for
//! timers and for work posted from other threads through a [`TaskSender`].
//! [`ClockMode::Virtual`] lets the loop skip idle gaps so long timers do not
//! hold up a page that is settling.
//!
//! Page scripts get `setTimeout`, `setInterval`, `clearTimeout`,
//! `clearInterval`, `queueMicrotask`, `requestAnimationFrame`,
//! `cancelAnimationFrame`, `reportError`, `performance` and a `console`
//! whose output is captured (see [`JsRuntime::take_console`]). `Date` reads
//! the same page clock as `performance.now()`.
//!
//! DOM events are implemented in JS glue evaluated after the prelude:
//! `EventTarget`, `Event`, `CustomEvent`, `ErrorEvent`, `DOMException`,
//! `AbortController` and `AbortSignal`. Each glue file receives the host
//! functions and a shared `internals` object (trusted event dispatch, `on*`
//! attributes, abort hooks, task queueing) for the glue that follows it.
//!
//! ```
//! use std::time::Duration;
//!
//! use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let js = JsRuntime::new(RuntimeOptions {
//!     clock: ClockMode::Manual,
//!     ..RuntimeOptions::default()
//! })?;
//! js.run_script("setTimeout(() => console.log('later', 42), 5000);", "page.js")?;
//! js.advance(Duration::from_secs(5))?;
//! assert_eq!(js.take_console()[0].text, "later 42");
//! assert_eq!(js.eval_json("performance.now()")?.as_deref(), Some("5000"));
//! # Ok(())
//! # }
//! ```
//!
//! # Network
//!
//! `fetch()` (with `Headers`, `Request`, `Response`, `Blob`, `File`,
//! `FormData` and `AbortSignal` support) and `XMLHttpRequest` send their
//! requests through the [`dasher_net::Session`] given as
//! [`RuntimeOptions::network`]. That way they share the page's cookie jar
//! and request policy, and they count towards the session's network-idle
//! tracking. Requests run as Tokio tasks and are cancelled when the runtime
//! is dropped. While one is in progress the event loop does not report
//! itself idle, and the virtual clock does not skip ahead. Relative URLs
//! resolve against the document's URL when they are sent, and the default
//! `same-origin` credentials mode sends cookies only to the document's own
//! origin. Without a session every request fails as a network error, except
//! `data:` URLs, which are always answered locally. Failed loads are logged
//! to the console with the reason.
//!
//! ```no_run
//! use dasher_js::{JsRuntime, RunLimits, RuntimeOptions};
//! use dasher_net::Client;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let session = Client::new()?.session();
//! let js = JsRuntime::new(RuntimeOptions {
//!     network: Some(session.clone()),
//!     ..RuntimeOptions::default()
//! })?;
//! js.run_script(
//!     "fetch('https://example.com/api').then((r) => r.json()).then((d) => globalThis.data = d);",
//!     "page.js",
//! )?;
//! js.run(RunLimits::default()).await;
//! let data = js.eval_json("data")?;
//! # let _ = data;
//! # Ok(())
//! # }
//! ```

mod clock;
mod console;
mod data_url;
mod encoding;
mod error;
mod event_loop;
mod glue;
mod heap;
mod log;
mod net;
mod random;
mod remote;
mod run;
mod runtime;
mod timers;
mod urls;

pub use clock::ClockMode;
pub use console::{ConsoleLevel, ConsoleMessage};
pub use error::{ErrorKind, ScriptError};
pub use event_loop::{DEFAULT_FRAME_INTERVAL, LoopHandle, Stopped, Task};
pub use glue::precompile;
pub use remote::{PendingOperation, SendTask, TaskSender};
pub use run::{RunLimits, RunOutcome};
pub use runtime::{
    DEFAULT_MAX_STACK_SIZE, DEFAULT_MEMORY_LIMIT, InterruptHandle, JsRuntime, RuntimeOptions,
};

/// The rquickjs version dasher is built against, for host bindings.
pub use rquickjs;
