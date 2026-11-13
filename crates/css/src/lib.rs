//! CSS selectors for dasher.
//!
//! Selector parsing and matching against the dasher DOM, used by
//! `querySelector`/`matches`/`closest` and by the extraction API. There is no
//! cascade, no computed style and no stylesheet loading.
//!
//! ```
//! use dasher_css::{SelectorList, query_selector_all};
//! use dasher_dom::Document;
//!
//! let doc = Document::parse(
//!     "<ul><li><a href=/a.pdf>A</a><li class=new><a href=/b>B</a><li>C</ul>",
//! );
//! let pdfs = query_selector_all(&doc, doc.root(), "li:has(a[href$='.pdf'])")?;
//! assert_eq!(pdfs.len(), 1);
//!
//! // Parse once, reuse across queries.
//! let links = SelectorList::parse("li:nth-child(n+2) > a, li.new")?;
//! let texts: Vec<_> = links
//!     .query_all(&doc, doc.root())
//!     .map(|n| doc.text_content(n).unwrap())
//!     .collect();
//! assert_eq!(texts, ["B", "B"]);
//!
//! let err = SelectorList::parse("li >").unwrap_err();
//! assert_eq!(err.name(), "SyntaxError");
//! # Ok::<(), dasher_css::SelectorError>(())
//! ```
//!
//! The selector engine is hand-written (a CSS Syntax tokenizer, a Selectors
//! Level 4 parser and a right-to-left matcher) rather than servo's
//! `selectors` crate, to keep the binary small and the matcher working
//! directly on dasher-dom's interned names.
//!
//! # What is supported
//!
//! - Type, universal, ID, class and attribute selectors, with namespace
//!   wildcards (`*|E`, `|E`, `[*|attr]`). There are no `@namespace` rules, so
//!   any other prefix is a syntax error. Type and attribute names match
//!   HTML elements case-insensitively and foreign (SVG, MathML) elements
//!   exactly; IDs and classes are case-insensitive only in quirks mode.
//! - Attribute operators `=`, `~=`, `|=`, `^=`, `$=`, `*=` and the `i`/`s`
//!   flags. HTML's legacy attributes (`type`, `lang`, `rel`...) compare
//!   values case-insensitively on HTML elements, as in browsers.
//! - All combinators: descendant, `>`, `+` and `~`.
//! - `:not()`, `:is()`, `:where()` (forgiving: invalid items are dropped)
//!   and `:has()` with relative selectors (`:has(> img)`, `:has(+ .x)`).
//! - `:root`, `:empty`, `:scope`, `:first-child`, `:last-child`,
//!   `:only-child`, `:first-of-type`, `:last-of-type`, `:only-of-type`,
//!   `:nth-child()`/`:nth-last-child()` (with `of S`), `:nth-of-type()`,
//!   `:nth-last-of-type()`.
//! - `:lang()`, `:dir()`, `:any-link`/`:link`, `:open`, `:defined`.
//!
//! # State dasher doesn't have
//!
//! dasher has no user, no focus, no history and no layout, so:
//!
//! - Form pseudo-classes are decided from the markup, as a freshly loaded
//!   page would show it: `:checked` (including a drop-down's default
//!   option), `:default`, `:indeterminate`, `:enabled`/`:disabled`
//!   (including disabled fieldsets), `:required`/`:optional`,
//!   `:read-only`/`:read-write` (including `contenteditable`) and
//!   `:placeholder-shown`. They read attributes, so changing an input's
//!   `checked` *property* without its attribute is not seen.
//! - `:hover`, `:active`, `:focus`, `:focus-within`, `:focus-visible`,
//!   `:visited`, `:target`, `:valid`/`:invalid`, the media-playback states
//!   and the like parse but never match. So do pseudo-elements (`::before`,
//!   `::-webkit-scrollbar`...), as in `querySelector`.
//! - `:defined` matches every element.
//! - The non-standard `:visible` (from jQuery and Playwright) is accepted
//!   for scraping convenience. With no layout it is decided statically:
//!   an element is visible unless the markup alone hides it (never-rendered
//!   elements such as `<head>` and `<script>`, the `hidden` attribute,
//!   inline `display: none` or `visibility: hidden`, closed `<dialog>`s and
//!   `<details>`, `<input type=hidden>`).
//!
//! # Speed
//!
//! A query doesn't always walk the whole tree: a selector with an `#id`
//! starts from the element with that id, through dasher-dom's id index,
//! and a selector of one compound (`li`, `.item`) skips the combinator
//! machinery. The `plan` module says which queries do what, and
//! `tests/plan.rs` checks every plan against matching each element.
//! `examples/query_timing.rs` times queries without the JS bindings.
//!
//! [`SelectorList::query_all_deep`] also looks into shadow trees, for the
//! host's own queries; a page's `querySelector` never does.
//!
//! Invalid selectors fail with a [`SelectorError`] whose
//! [`name`](SelectorError::name) is `"SyntaxError"`, wherever a browser would
//! throw one. Like the CSS parser, the tokenizer closes blocks and strings
//! left open at the end of input, so `a[href` is valid.

mod error;
mod matching;
mod parser;
mod plan;
mod selector;
mod tokenizer;

use dasher_dom::{Document, NodeId, ShadowIncluding};

pub use error::SelectorError;
pub use selector::SelectorList;

use matching::Context;

impl SelectorList {
    /// Parses a comma-separated selector list.
    ///
    /// Fails with a `SyntaxError`-shaped [`SelectorError`] where a browser's
    /// `querySelector` would throw one.
    pub fn parse(selectors: &str) -> Result<SelectorList, SelectorError> {
        parser::parse(selectors)
    }

    /// The number of selectors in the list.
    pub fn len(&self) -> usize {
        self.selectors.len()
    }

    /// Whether the list is empty. Only a nested list can be: `:is()`.
    pub fn is_empty(&self) -> bool {
        self.selectors.is_empty()
    }

    /// `Element.matches`: whether `element` matches any selector in the
    /// list, with `:scope` matching `element` itself. False for non-elements.
    pub fn matches(&self, doc: &Document, element: NodeId) -> bool {
        self.matches_in_scope(doc, element, Some(element))
    }

    /// Whether `element` matches, with `:scope` matching `scope`. `None`,
    /// or the document node, makes `:scope` match the root element, as in a
    /// stylesheet. False for non-elements.
    pub fn matches_in_scope(&self, doc: &Document, element: NodeId, scope: Option<NodeId>) -> bool {
        doc.is_element(element) && Context::new(doc, scope).matches_list(self, element)
    }

    /// `Element.closest`: the nearest inclusive ancestor element of
    /// `element` that matches, with `:scope` matching `element`.
    pub fn closest(&self, doc: &Document, element: NodeId) -> Option<NodeId> {
        if !doc.is_element(element) {
            return None;
        }
        let cx = Context::new(doc, Some(element));
        std::iter::successors(Some(element), |&n| doc.parent_element(n))
            .find(|&n| cx.matches_list(self, n))
    }

    /// `querySelector`: the first descendant element of `root`, in tree
    /// order, that matches. It stops at the first match.
    pub fn query_first(&self, doc: &Document, root: NodeId) -> Option<NodeId> {
        self.query_all(doc, root).next()
    }

    /// `querySelectorAll`: the descendant elements of `root` that match, in
    /// tree order, as a lazy iterator.
    ///
    /// `:scope` matches `root` (the root element when `root` is the
    /// document). As in browsers, the rest of the selector is not confined
    /// to `root`'s subtree: `div.querySelectorAll("body p")` finds the `p`s
    /// in the `div` even though `body` is outside it.
    ///
    /// A selector with an `#id` starts from the element with that id (see
    /// the `plan` module) instead of walking the whole of `root`.
    pub fn query_all<'a>(&'a self, doc: &'a Document, root: NodeId) -> QueryAll<'a> {
        let cx = Context::new(doc, Some(root));
        QueryAll {
            nodes: plan::source(self, &cx, root),
            test: plan::test(self, doc),
            cx,
        }
    }

    /// Like [`query_first`](Self::query_first), but also looking inside
    /// shadow trees, open or closed: see [`query_all_deep`](Self::query_all_deep).
    pub fn query_first_deep(&self, doc: &Document, root: NodeId) -> Option<NodeId> {
        self.query_all_deep(doc, root).next()
    }

    /// Like [`query_all`](Self::query_all), but also looking inside shadow
    /// trees, open or closed, as a scraper wants and as Playwright's CSS
    /// engine does. Elements come in shadow-including tree order (a host's
    /// shadow tree before its children), and the descendant and child
    /// combinators cross from a shadow tree to its host, so `my-card h2`
    /// finds an `h2` in `my-card`'s shadow tree. Pages' own
    /// `querySelector` never looks into shadow trees.
    pub fn query_all_deep<'a>(&'a self, doc: &'a Document, root: NodeId) -> QueryAllDeep<'a> {
        let mut nodes = doc.shadow_including_inclusive_descendants(root);
        nodes.next();
        QueryAllDeep {
            cx: Context::deep(doc, Some(root)),
            list: self,
            nodes,
        }
    }
}

/// The matches of [`SelectorList::query_all_deep`], in shadow-including
/// tree order.
pub struct QueryAllDeep<'a> {
    cx: Context<'a>,
    list: &'a SelectorList,
    nodes: ShadowIncluding<'a>,
}

impl Iterator for QueryAllDeep<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let doc = self.cx.doc;
        self.nodes
            .by_ref()
            .find(|&n| doc.is_element(n) && self.cx.matches_list(self.list, n))
    }
}

/// The matches of [`SelectorList::query_all`], in tree order.
pub struct QueryAll<'a> {
    cx: Context<'a>,
    test: plan::Test<'a>,
    nodes: plan::Source<'a>,
}

impl Iterator for QueryAll<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let doc = self.cx.doc;
        self.nodes
            .by_ref()
            .find(|&n| doc.is_element(n) && self.test.matches(&self.cx, n))
    }
}

/// `root.querySelector(selectors)`.
pub fn query_selector(
    doc: &Document,
    root: NodeId,
    selectors: &str,
) -> Result<Option<NodeId>, SelectorError> {
    Ok(SelectorList::parse(selectors)?.query_first(doc, root))
}

/// `root.querySelectorAll(selectors)`, collected.
pub fn query_selector_all(
    doc: &Document,
    root: NodeId,
    selectors: &str,
) -> Result<Vec<NodeId>, SelectorError> {
    Ok(SelectorList::parse(selectors)?
        .query_all(doc, root)
        .collect())
}

/// `element.matches(selectors)`.
pub fn matches(doc: &Document, element: NodeId, selectors: &str) -> Result<bool, SelectorError> {
    Ok(SelectorList::parse(selectors)?.matches(doc, element))
}

/// `element.closest(selectors)`.
pub fn closest(
    doc: &Document,
    element: NodeId,
    selectors: &str,
) -> Result<Option<NodeId>, SelectorError> {
    Ok(SelectorList::parse(selectors)?.closest(doc, element))
}
