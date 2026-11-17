//! The parsed form of a selector list.
//!
//! Complex selectors are stored right to left, the order they are matched
//! in. Names are stored as strings unless they are one of dasher-dom's
//! well-known names, which have the same [`Name`] in every document and can
//! be compared as integers. That keeps a parsed selector independent of any
//! one document, so it can be cached and reused.

use dasher_dom::Name;

/// A parsed selector list, such as `"a.next, button[rel=next]"`.
///
/// Parse once with [`SelectorList::parse`] and reuse it across queries and
/// documents.
#[derive(Clone, Debug)]
pub struct SelectorList {
    pub(crate) selectors: Box<[Complex]>,
}

/// A complex selector: compounds joined by combinators, stored right to
/// left. `parts[0]` is the subject; `parts[k].combinator` joins `parts[k]`
/// to `parts[k + 1]`, the compound on its left.
#[derive(Clone, Debug)]
pub(crate) struct Complex {
    pub(crate) parts: Box<[Part]>,
}

impl Complex {
    /// See [`SelectorList::reads_text`].
    pub(crate) fn reads_text(&self) -> bool {
        self.parts
            .iter()
            .flat_map(|part| part.compound.iter())
            .any(|simple| match simple {
                Simple::Pseudo(Pseudo::Empty | Pseudo::PlaceholderShown) => true,
                Simple::Pseudo(Pseudo::Not(list) | Pseudo::Is(list)) => list.reads_text(),
                Simple::Pseudo(Pseudo::Has(relatives)) => {
                    relatives.iter().any(|r| r.complex.reads_text())
                }
                Simple::Pseudo(Pseudo::Nth(nth)) => {
                    nth.of.as_ref().is_some_and(SelectorList::reads_text)
                }
                _ => false,
            })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Part {
    pub(crate) compound: Box<[Simple]>,
    /// Unused on the leftmost part.
    pub(crate) combinator: Combinator,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Combinator {
    /// Whitespace.
    Descendant,
    /// `>`
    Child,
    /// `+`
    NextSibling,
    /// `~`
    SubsequentSibling,
}

impl Combinator {
    pub(crate) fn is_sibling(self) -> bool {
        matches!(
            self,
            Combinator::NextSibling | Combinator::SubsequentSibling
        )
    }
}

/// A name in a selector, resolved as far as is possible without a document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum NameRef {
    /// A well-known name, the same in every document.
    Static(Name),
    /// Any other name. No element or attribute with a well-known name can
    /// match it.
    Other(Box<str>),
}

/// The local name of a type or attribute selector. In an HTML document,
/// names match HTML elements (and their attributes) ASCII
/// case-insensitively, so the lower-cased form is kept for those.
#[derive(Clone, Debug)]
pub(crate) struct LocalName {
    pub(crate) lower: NameRef,
    pub(crate) exact: NameRef,
}

#[derive(Clone, Debug)]
pub(crate) enum Simple {
    /// A type selector. `local: None` is `*`; `no_namespace` is `|E`. The
    /// plain `*` and `*|*` match everything and are left out instead.
    Type {
        no_namespace: bool,
        local: Option<LocalName>,
    },
    Id(Box<str>),
    Class(Box<str>),
    Attr(Box<AttrSelector>),
    Pseudo(Pseudo),
    /// The element a relative selector in `:has()` is anchored to.
    Anchor,
    /// Something that never matches: a pseudo-element, or a pseudo-class
    /// for state dasher does not have (`:hover`, `:focus`, ...).
    Never,
}

#[derive(Clone, Debug)]
pub(crate) struct AttrSelector {
    /// `[*|name]`. Otherwise only attributes with no namespace match.
    pub(crate) any_namespace: bool,
    pub(crate) local: LocalName,
    /// The operator and value; `None` for `[name]`.
    pub(crate) value: Option<(AttrOp, Box<str>)>,
    pub(crate) case: AttrCase,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AttrOp {
    /// `=`
    Equals,
    /// `~=`
    Includes,
    /// `|=`
    DashMatch,
    /// `^=`
    Prefix,
    /// `$=`
    Suffix,
    /// `*=`
    Substring,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AttrCase {
    Sensitive,
    /// The `i` flag.
    Insensitive,
    /// One of HTML's legacy attributes (`type`, `lang`, ...) whose values
    /// match case-insensitively on HTML elements unless the `s` flag is set.
    HtmlLegacy,
}

#[derive(Clone, Debug)]
pub(crate) enum Pseudo {
    Root,
    Empty,
    Scope,
    OnlyChild,
    OnlyOfType,
    /// `:nth-child()` and friends, including `:first-child` (`0n+1`) etc.
    Nth(Box<Nth>),
    Not(SelectorList),
    /// `:is()` and `:where()`.
    Is(SelectorList),
    Has(Box<[Relative]>),
    /// `:any-link` and `:link`; nothing is ever visited.
    AnyLink,
    Enabled,
    Disabled,
    Checked,
    Indeterminate,
    Default,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Open,
    Defined,
    /// Non-standard (jQuery, Playwright): see `matching::is_visible`.
    Visible,
    Lang(Box<[Box<str>]>),
    /// `:dir(ltr)` is `Some(false)`, `:dir(rtl)` is `Some(true)`; any other
    /// direction is valid but matches nothing.
    Dir(Option<bool>),
}

#[derive(Clone, Debug)]
pub(crate) struct Nth {
    pub(crate) a: i32,
    pub(crate) b: i32,
    /// Count from the last sibling rather than the first.
    pub(crate) from_end: bool,
    /// Only count siblings with the same type (`:nth-of-type`).
    pub(crate) of_type: bool,
    /// Only count siblings matching these (`:nth-child(An+B of S)`).
    pub(crate) of: Option<SelectorList>,
}

impl Nth {
    /// Whether the 1-based `index` is `a*n + b` for some `n >= 0`.
    pub(crate) fn matches_index(&self, index: i64) -> bool {
        let (a, b) = (i64::from(self.a), i64::from(self.b));
        if a == 0 {
            index == b
        } else {
            let diff = index - b;
            diff % a == 0 && diff / a >= 0
        }
    }
}

/// A relative selector in `:has()`. Its leftmost part is an
/// [`Simple::Anchor`].
#[derive(Clone, Debug)]
pub(crate) struct Relative {
    pub(crate) complex: Complex,
    /// Which elements around the anchor can match.
    pub(crate) reach: Reach,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Reach {
    Children,
    Descendants,
    NextSibling,
    Siblings,
    /// Later siblings and their descendants, e.g. `:has(~ a b)`.
    SiblingsAndDescendants,
}
