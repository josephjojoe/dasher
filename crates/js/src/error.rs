//! Errors raised by page scripts, captured instead of propagated.

use std::fmt;

/// What kind of failure a [`ScriptError`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An exception escaped a script, callback, task or microtask, and no
    /// listener canceled the `error` event fired for it at the global.
    Uncaught,
    /// A promise was rejected and still had no handler at the end of the
    /// microtask checkpoint that followed, and no listener canceled the
    /// `unhandledrejection` event fired for it at the global.
    UnhandledRejection,
    /// The runtime hit a memory limit: an allocation failed, or the page
    /// went over one of its [resource limits](crate::ResourceLimit).
    OutOfMemory,
    /// Execution was cut off by the deadline or an [`InterruptHandle`].
    ///
    /// [`InterruptHandle`]: crate::InterruptHandle
    Interrupted,
    /// The engine or a host binding failed for a reason other than a JS
    /// exception (for example a value that could not be converted).
    Internal,
}

/// An error raised while running page JavaScript.
///
/// Errors never unwind into the host: they are recorded in the runtime's
/// error log (see [`JsRuntime::take_errors`]) and, where the host asked for a
/// result, also returned.
///
/// [`JsRuntime::take_errors`]: crate::JsRuntime::take_errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// A one-line description, such as `TypeError: x is not a function`.
    pub message: String,
    /// The JS stack trace at the throw site, when the thrown value had one.
    pub stack: Option<String>,
}

impl ScriptError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            stack: None,
        }
    }

    pub(crate) fn interrupted() -> Self {
        Self::new(ErrorKind::Interrupted, "script execution was interrupted")
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Uncaught => write!(f, "Uncaught {}", self.message),
            ErrorKind::UnhandledRejection => write!(f, "Uncaught (in promise) {}", self.message),
            ErrorKind::OutOfMemory | ErrorKind::Interrupted | ErrorKind::Internal => {
                f.write_str(&self.message)
            }
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_match_browser_console_wording() {
        let e = ScriptError::new(ErrorKind::Uncaught, "Error: boom");
        assert_eq!(e.to_string(), "Uncaught Error: boom");
        let e = ScriptError::new(ErrorKind::UnhandledRejection, "Error: nope");
        assert_eq!(e.to_string(), "Uncaught (in promise) Error: nope");
        assert_eq!(
            ScriptError::interrupted().to_string(),
            "script execution was interrupted"
        );
    }
}
