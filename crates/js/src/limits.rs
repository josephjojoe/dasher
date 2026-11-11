//! The per-page resource limits that stop a page outright.

use std::fmt;

/// Default for [`RuntimeOptions::max_dom_nodes`](crate::RuntimeOptions::max_dom_nodes).
///
/// Real pages stay well below it (a 5000-row table is about 75,000 nodes),
/// and at roughly 130 bytes of RSS per parsed node it caps a runaway
/// document at about 65 MiB.
pub const DEFAULT_MAX_DOM_NODES: usize = 500_000;

/// A per-page limit a page went over.
///
/// Going over one stops the page for good: running JS is cut off at its
/// next interrupt check, as it is at a deadline, and no more JS runs on
/// that runtime. [`JsRuntime::limit_exceeded`](crate::JsRuntime::limit_exceeded)
/// says which limit it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceLimit {
    /// The JS heap filled up: an allocation was refused while more than
    /// half of [`RuntimeOptions::memory_limit`](crate::RuntimeOptions::memory_limit)
    /// was in use. (A single absurd request on a small heap, such as
    /// `new ArrayBuffer(2 ** 40)`, only throws, as in a browser.)
    JsHeap {
        /// The limit, in bytes.
        limit: usize,
    },
    /// The document held more nodes than
    /// [`RuntimeOptions::max_dom_nodes`](crate::RuntimeOptions::max_dom_nodes).
    DomNodes {
        /// The limit, in nodes.
        limit: usize,
    },
}

impl fmt::Display for ResourceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JsHeap { limit } => write!(
                f,
                "the page ran out of JS heap (limit {:.0} MiB)",
                *limit as f64 / (1024.0 * 1024.0)
            ),
            Self::DomNodes { limit } => {
                write!(f, "the page went over its limit of {limit} DOM nodes")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        assert_eq!(
            ResourceLimit::JsHeap { limit: 64 << 20 }.to_string(),
            "the page ran out of JS heap (limit 64 MiB)"
        );
        assert_eq!(
            ResourceLimit::DomNodes { limit: 1000 }.to_string(),
            "the page went over its limit of 1000 DOM nodes"
        );
    }
}
