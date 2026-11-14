//! JSON forms of what a page load reports, shared by the service and the
//! CLI's `--json` output.

use dasher::{
    AutoScrollReport, AutoScrollStop, ConsoleLevel, ConsoleMessage, Error, JsonValue, ScriptError,
    ScriptErrorKind, SettleOutcome,
};
use serde_json::json;

/// A console level's name: `"log"`, `"error"` and so on.
pub fn level_name(level: ConsoleLevel) -> &'static str {
    match level {
        ConsoleLevel::Debug => "debug",
        ConsoleLevel::Log => "log",
        ConsoleLevel::Info => "info",
        ConsoleLevel::Warn => "warn",
        ConsoleLevel::Error => "error",
    }
}

/// `{"level": …, "text": …}`.
pub fn console_json(message: &ConsoleMessage) -> JsonValue {
    json!({
        "level": level_name(message.level),
        "text": message.text,
    })
}

/// How settling ended, as a snake_case name.
pub fn settle_name(outcome: SettleOutcome) -> &'static str {
    match outcome {
        SettleOutcome::Settled => "settled",
        SettleOutcome::TimedOut => "timed_out",
        SettleOutcome::NavigationRequested => "navigation_requested",
        SettleOutcome::Interrupted => "interrupted",
        SettleOutcome::LimitExceeded(_) => "limit_exceeded",
        SettleOutcome::BudgetExhausted(_) => "budget_exhausted",
    }
}

/// `{"scrolls": …, "productive_scrolls": …, "stopped": …}`.
pub fn scroll_json(report: &AutoScrollReport) -> JsonValue {
    let stopped = match report.stopped {
        AutoScrollStop::NoNewContent => "no_new_content",
        AutoScrollStop::ScrollLimit => "scroll_limit",
        AutoScrollStop::TimedOut => "timed_out",
        AutoScrollStop::NavigationRequested => "navigation_requested",
        AutoScrollStop::Interrupted => "interrupted",
        AutoScrollStop::LimitExceeded(_) => "limit_exceeded",
        AutoScrollStop::BudgetExhausted(_) => "budget_exhausted",
    };
    json!({
        "scrolls": report.scrolls,
        "productive_scrolls": report.productive_scrolls,
        "stopped": stopped,
    })
}

/// `{"kind": …, "message": …, "stack": …}` for an uncaught script error.
pub fn error_json(error: &ScriptError) -> JsonValue {
    let kind = match error.kind {
        ScriptErrorKind::Uncaught => "uncaught",
        ScriptErrorKind::UnhandledRejection => "unhandled_rejection",
        ScriptErrorKind::OutOfMemory => "out_of_memory",
        ScriptErrorKind::Interrupted => "interrupted",
        _ => "internal",
    };
    json!({
        "kind": kind,
        "message": error.message,
        "stack": error.stack,
    })
}

/// An error's message, with the JS stack when there is one.
pub fn error_message(error: &Error) -> String {
    match error {
        Error::Script(e) => match &e.stack {
            Some(stack) => format!("{e}\n{}", stack.trim_end()),
            None => e.to_string(),
        },
        other => other.to_string(),
    }
}
