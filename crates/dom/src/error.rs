use std::fmt;

/// A failed DOM operation. Each variant corresponds to the `DOMException`
/// name the JS bindings throw for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DomError {
    /// `HierarchyRequestError`: the operation would yield an incorrect tree,
    /// e.g. inserting a node into its own descendant or a second root
    /// element into a document.
    HierarchyRequest,
    /// `NotFoundError`: the reference node is not a child of the parent.
    NotFound,
    /// `NoModificationAllowedError`: e.g. setting `outerHTML` on the
    /// document element.
    NoModificationAllowed,
}

impl DomError {
    /// The `DOMException` name for this error.
    pub fn name(self) -> &'static str {
        match self {
            DomError::HierarchyRequest => "HierarchyRequestError",
            DomError::NotFound => "NotFoundError",
            DomError::NoModificationAllowed => "NoModificationAllowedError",
        }
    }
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            DomError::HierarchyRequest => "the operation would yield an incorrect node tree",
            DomError::NotFound => "the node is not a child of this node",
            DomError::NoModificationAllowed => "the node cannot be modified",
        };
        write!(f, "{}: {msg}", self.name())
    }
}

impl std::error::Error for DomError {}
