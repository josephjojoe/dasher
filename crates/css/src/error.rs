use std::fmt;

/// A selector that failed to parse.
///
/// The JS bindings throw it as a `DOMException` named
/// [`name()`](Self::name), i.e. a `SyntaxError`, as browsers do for
/// `querySelector("div[")`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SelectorError {
    selector: Box<str>,
    reason: &'static str,
}

impl SelectorError {
    pub(crate) fn new(selector: &str, reason: &'static str) -> Self {
        SelectorError {
            selector: selector.into(),
            reason,
        }
    }

    /// The `DOMException` name: always `"SyntaxError"`.
    pub fn name(&self) -> &'static str {
        "SyntaxError"
    }

    /// The selector text that failed to parse.
    pub fn selector(&self) -> &str {
        &self.selector
    }

    /// A short description of what was wrong.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' is not a valid selector: {}",
            self.selector, self.reason
        )
    }
}

impl std::error::Error for SelectorError {}
