//! Captured `console` output.

use std::rc::Rc;
use std::time::Duration;

use rquickjs::{Ctx, Function, Object};

use crate::JsRuntime;
use crate::runtime::Shared;

/// Severity of a console message, after the `console` method that wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConsoleLevel {
    /// `console.debug`.
    Debug,
    /// `console.log`, `trace`, `dir`, `table`, `group`, `count` and `time*`.
    Log,
    /// `console.info`.
    Info,
    /// `console.warn`.
    Warn,
    /// `console.error` and failed `console.assert`.
    Error,
}

impl ConsoleLevel {
    fn from_name(name: &str) -> Self {
        match name {
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Log,
        }
    }
}

/// One message written to the page's `console`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleMessage {
    /// Which kind of `console` method wrote it.
    pub level: ConsoleLevel,
    /// The arguments rendered to text the way a browser console shows them,
    /// with `%s`-style substitutions applied.
    pub text: String,
    /// Page time at which it was written.
    pub time: Duration,
}

/// Cuts `text` to at most `max` bytes on a character boundary, marking the
/// cut with an ellipsis.
fn truncate(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

/// Adds the console's host function to the prelude's `native` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
    max_message_len: usize,
) -> rquickjs::Result<()> {
    let s = Rc::clone(shared);
    native.set(
        "log",
        Function::new(ctx.clone(), move |level: String, text: String| {
            s.console.borrow_mut().push(ConsoleMessage {
                level: ConsoleLevel::from_name(&level),
                text: truncate(text, max_message_len),
                time: s.event_loop.clock.now(),
            });
        })?,
    )
}

impl JsRuntime {
    /// The console messages captured so far, oldest first.
    pub fn console(&self) -> Vec<ConsoleMessage> {
        self.shared().console.borrow().iter().cloned().collect()
    }

    /// Removes and returns the captured console messages, oldest first.
    pub fn take_console(&self) -> Vec<ConsoleMessage> {
        self.shared().console.borrow_mut().take()
    }

    /// How many console messages were discarded because the log was full.
    pub fn dropped_console_messages(&self) -> u64 {
        self.shared().console.borrow().dropped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("short".into(), 10), "short");
        assert_eq!(truncate("abcdef".into(), 3), "abc…");
        // 'é' is two bytes; cutting inside it backs off to the boundary.
        assert_eq!(truncate("aé".into(), 2), "a…");
    }

    #[test]
    fn levels_from_method_names() {
        assert_eq!(ConsoleLevel::from_name("warn"), ConsoleLevel::Warn);
        assert_eq!(ConsoleLevel::from_name("log"), ConsoleLevel::Log);
        assert_eq!(ConsoleLevel::from_name("other"), ConsoleLevel::Log);
        assert!(ConsoleLevel::Debug < ConsoleLevel::Error);
    }
}
