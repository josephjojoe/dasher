//! The synthetic layout's raw material, and the hook that tells layout
//! observers the document changed.
//!
//! Nothing is laid out. Where geometry matters (the `scrolled` layout mode,
//! `scrollIntoView`, `IntersectionObserver`), `dom.js` stacks the elements
//! under `<body>` in document order, one line each, with every element
//! spanning the lines of its descendants. [`lines`] gives an element's place
//! in that stack.
//!
//! `IntersectionObserver` must look again whenever the tree changes. Rather
//! than have every mutation call into JS, the event loop calls the layout
//! hook (set with `native.dom.setLayoutHook`) once the task queue has
//! drained, if the document changed since the hook last ran: much like a
//! browser updating intersections in its rendering step.

use std::collections::HashMap;

use dasher_dom::{Document, NodeId};

use super::DomState;
use super::wrapper::DomData;
use crate::runtime::JsRuntime;

/// Each target's place in the stacked layout, as `[line, lines]` pairs in
/// one flat list: the element's index among the elements under `<body>` in
/// document order (`<body>` itself is line 0), and how many lines it spans
/// (itself and its element descendants). Elements outside `<body>`, and
/// documents without one, get `[-1, 0]`.
pub(crate) fn lines(doc: &Document, targets: &[NodeId]) -> Vec<i32> {
    let mut out = [-1, 0].repeat(targets.len());
    let Some(body) = doc.body() else {
        return out;
    };
    let mut wanted: HashMap<NodeId, Vec<usize>> = HashMap::with_capacity(targets.len());
    for (i, &t) in targets.iter().enumerate() {
        wanted.entry(t).or_default().push(i);
    }
    let mut record = |node: NodeId, line: i32, end: i32| {
        if let Some(indices) = wanted.get(&node) {
            for &i in indices {
                out[2 * i] = line;
                out[2 * i + 1] = end - line;
            }
        }
    };
    // Depth-first, keeping for each open element its next child to visit
    // and its line; an element's span is known when it is closed.
    let mut next = 0i32;
    let mut open = vec![(body, doc.first_child(body), next)];
    next += 1;
    while let Some(top) = open.last_mut() {
        match top.1 {
            Some(child) => {
                top.1 = doc.next_sibling(child);
                if doc.is_element(child) {
                    open.push((child, doc.first_child(child), next));
                    next += 1;
                }
            }
            None => {
                let (node, _, line) = open.pop().expect("not empty");
                record(node, line, next);
            }
        }
    }
    out
}

impl DomState {
    /// Sets whether the layout hook wants to hear about changes, counting
    /// the document as seen as it is now.
    pub(crate) fn watch_layout(&self, watch: bool) {
        self.layout_watch.set(watch);
        self.layout_seen.set(self.version());
    }

    /// Whether the layout hook is waiting to hear about a change.
    pub(crate) fn layout_changed(&self) -> bool {
        self.layout_watch.get() && self.layout_seen.get() != self.version()
    }
}

impl JsRuntime {
    /// Calls the layout hook, followed by a microtask checkpoint, if the
    /// document changed since it last ran. Returns whether it ran.
    pub(crate) fn update_layout(&self) -> bool {
        let dom = &self.shared().dom;
        if !dom.layout_changed() {
            return false;
        }
        dom.layout_seen.set(dom.version());
        let _ = self.run_task(|ctx| {
            let hook = ctx
                .userdata::<DomData<'_>>()
                .and_then(|d| d.layout_hook.borrow().clone());
            match hook {
                Some(hook) => hook.call(()),
                None => Ok(()),
            }
        });
        self.perform_microtask_checkpoint();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(doc: &Document, id: &str) -> NodeId {
        doc.get_element_by_id(doc.root(), id).unwrap()
    }

    #[test]
    fn stacks_elements_in_document_order() {
        let doc = Document::parse(
            "<head><title id=t>x</title></head><body>\
             <div id=a><p id=b>one</p>text<p id=c><b id=d>two</b></p></div>\
             <footer id=e></footer></body>",
        );
        let ids: Vec<NodeId> = ["a", "b", "c", "d", "e", "t", "a"]
            .iter()
            .map(|id| find(&doc, id))
            .collect();
        let body = doc.body().unwrap();
        assert_eq!(
            lines(&doc, &ids),
            [1, 4, 2, 1, 3, 2, 4, 1, 5, 1, -1, 0, 1, 4]
        );
        assert_eq!(lines(&doc, &[body]), [0, 6]);
    }

    #[test]
    fn no_body() {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        let root = doc.root();
        doc.append_child(root, html).unwrap();
        assert_eq!(lines(&doc, &[html]), [-1, 0]);
    }
}
