//! Query plans: where a `querySelector*` query looks for candidates, and
//! how it tests each one.
//!
//! The general plan walks every descendant of the query's root and runs
//! the full matcher on each element. Three things let a query do less:
//!
//! - A tag or attribute name the document has never used. Elements and
//!   attributes hold their names as entries of the document's name table,
//!   so a name that is not in the table is on no node. If every selector
//!   in the list has such a name in one of its compounds (`my-widget`,
//!   `[data-tracker-id="7"] a`), nothing can match and there is no walk.
//!   Scripts probe for markup that is not there far more often than for
//!   markup that is.
//! - An `#id` in the selector. The document's id index gives the elements
//!   with that id directly (see `Document::elements_with_id`). If every
//!   selector in the list has an id in its subject (rightmost) compound,
//!   the candidates are just those elements. If the one selector has an id
//!   in a compound further left, joined to the subject only by descendant
//!   and child combinators, every match is a descendant of the element
//!   with that id, so the walk starts there instead of at the root.
//! - A selector of a single compound (`li`, `.item`, `a[href]`), which
//!   needs none of the combinator machinery, and a lone type selector,
//!   whose name is looked up in the document's name table once per query
//!   so each element costs one integer comparison.
//!
//! Plans only narrow the candidates: each candidate is still tested
//! against the whole selector, so a plan can be conservative but never
//! wrong. Queries that look into shadow trees always use the general plan.
//!
//! There is no class or tag index, and no ancestor filter: walking the
//! tree is already fast for those (about 10 ns per element), and an index
//! would cost memory on every page for the few that query them.

use dasher_dom::{Descendants, Document, Name, NodeId};

use crate::matching::Context;
use crate::selector::{Complex, LocalName, NameRef, SelectorList, Simple};

/// Where a query's candidates come from.
pub(crate) enum Source<'a> {
    /// The descendants of a node, in tree order.
    Tree(Descendants<'a>),
    /// No candidates, or just one.
    One(Option<NodeId>),
    /// A few candidates, in tree order.
    Nodes(std::vec::IntoIter<NodeId>),
}

impl Iterator for Source<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        match self {
            Source::Tree(nodes) => nodes.next(),
            Source::One(node) => node.take(),
            Source::Nodes(nodes) => nodes.next(),
        }
    }
}

/// How a candidate is tested.
pub(crate) enum Test<'a> {
    /// The full matcher, for any selector list.
    List(&'a SelectorList),
    /// One compound with no combinators.
    Compound(&'a [Simple]),
    /// A lone type selector, its names resolved against the document:
    /// `None` is a name the document has never used, which no element can
    /// have.
    Tag {
        no_namespace: bool,
        html: Option<Name>,
        other: Option<Name>,
    },
}

impl Test<'_> {
    /// Whether the element `el` passes.
    pub(crate) fn matches(&self, cx: &Context<'_>, el: NodeId) -> bool {
        match self {
            Test::List(list) => cx.matches_list(list, el),
            Test::Compound(compound) => cx.matches_compound(compound, el),
            &Test::Tag {
                no_namespace,
                html,
                other,
            } => {
                let doc = cx.doc;
                let (Some(ns), Some(local)) = (doc.namespace(el), doc.local_name(el)) else {
                    return false;
                };
                let wanted = if ns == Name::NS_HTML { html } else { other };
                wanted == Some(local) && (!no_namespace || ns == Name::EMPTY)
            }
        }
    }
}

/// How to test candidates for `list` in `doc`.
pub(crate) fn test<'a>(list: &'a SelectorList, doc: &Document) -> Test<'a> {
    let [complex] = &*list.selectors else {
        return Test::List(list);
    };
    let [part] = &*complex.parts else {
        return Test::List(list);
    };
    match &*part.compound {
        [
            Simple::Type {
                no_namespace,
                local: Some(local),
            },
        ] => {
            let resolve = |name: &NameRef| match name {
                NameRef::Static(name) => Some(*name),
                // The parser makes every well-known name `Static`, so a
                // match here is a name of the document's own.
                NameRef::Other(name) => doc.names().get(name),
            };
            Test::Tag {
                no_namespace: *no_namespace,
                html: resolve(&local.lower),
                other: resolve(&local.exact),
            }
        }
        compound => Test::Compound(compound),
    }
}

/// Where to look for the elements under `root` that match `list`.
pub(crate) fn source<'a>(list: &SelectorList, cx: &Context<'a>, root: NodeId) -> Source<'a> {
    let doc = cx.doc;
    let quirks = cx.quirks;

    // Every selector needs a name that is on no node: nothing matches.
    if list
        .selectors
        .iter()
        .all(|complex| needs_unused_name(complex, doc))
    {
        debug_assert!(
            doc.descendants(root)
                .all(|n| !doc.is_element(n) || !cx.matches_list(list, n)),
            "a selector with an unused name matched"
        );
        return Source::One(None);
    }

    // Every selector's subject has an id: the candidates are the elements
    // with those ids.
    if list
        .selectors
        .iter()
        .all(|c| compound_id(&c.parts[0].compound).is_some())
    {
        let mut found = list
            .selectors
            .iter()
            .filter_map(|c| compound_id(&c.parts[0].compound))
            .flat_map(|id| doc.elements_with_id(id, quirks))
            .filter(|&n| doc.is_descendant(n, root));
        let first = found.next();
        let Some(second) = found.next() else {
            return Source::One(first);
        };
        let mut nodes: Vec<NodeId> = [first.expect("came first"), second]
            .into_iter()
            .chain(found)
            .collect();
        nodes.sort_unstable_by(|&a, &b| {
            doc.compare_tree_order(a, b)
                .expect("all under the query's root")
        });
        nodes.dedup();
        return Source::Nodes(nodes.into_iter());
    }

    // One selector with an id further left: start from that element.
    if let [complex] = &*list.selectors
        && let Some(id) = ancestor_id(complex)
    {
        let mut found = doc.elements_with_id(id, quirks);
        match (found.next(), found.next()) {
            // No element has the id, so nothing matches.
            (None, _) => return Source::One(None),
            (Some(e), None) if doc.is_descendant(e, root) => {
                return Source::Tree(doc.descendants(e));
            }
            // The element is the root or above it, so it narrows
            // nothing; or it is in another tree, so nothing matches
            // (but a full walk finds that out too).
            (Some(_), None) => {}
            // A repeated id: walk everything.
            (Some(_), Some(_)) => {}
        }
    }

    Source::Tree(doc.descendants(root))
}

/// Whether `complex` cannot match in `doc` because one of its compounds
/// requires a tag or attribute name that is not in the document's name
/// table, in the form it is compared in for an HTML element or in the form
/// for any other. Only a compound's own simple selectors count: a name
/// inside `:not()`, `:is()` or `:has()` is not required.
fn needs_unused_name(complex: &Complex, doc: &Document) -> bool {
    let unused = |name: &NameRef| match name {
        NameRef::Static(_) => false,
        NameRef::Other(name) => doc.names().get(name).is_none(),
    };
    let unused = |local: &LocalName| unused(&local.lower) && unused(&local.exact);
    complex.parts.iter().any(|part| {
        part.compound.iter().any(|simple| match simple {
            Simple::Type {
                local: Some(local), ..
            } => unused(local),
            Simple::Attr(attr) => unused(&attr.local),
            _ => false,
        })
    })
}

/// The id a compound requires, if any.
fn compound_id(compound: &[Simple]) -> Option<&str> {
    compound.iter().find_map(|s| match s {
        Simple::Id(id) => Some(&**id),
        _ => None,
    })
}

/// The id of the nearest compound left of the subject that every match
/// must be a descendant of: one reached from the subject through
/// descendant and child combinators only.
fn ancestor_id(complex: &Complex) -> Option<&str> {
    let parts = &complex.parts;
    for k in 1..parts.len() {
        if parts[k - 1].combinator.is_sibling() {
            return None;
        }
        if let Some(id) = compound_id(&parts[k].compound) {
            return Some(id);
        }
    }
    None
}
