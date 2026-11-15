//! Document order: `compareDocumentPosition` and the node document, fast
//! enough to sort by.
//!
//! Sorting nodes into document order asks both for every comparison, and
//! jQuery sorts thousands of nodes at a time: a page can ask a million times
//! between two changes to its tree. Each answer is a walk up the tree (and
//! along a child list), so once the walks made since the tree last changed
//! add up to as many steps as numbering the whole document would take, the
//! document is numbered, and until its next change every answer about two of
//! its nodes is a comparison of two numbers.
//!
//! Only the page's document is numbered. Its nodes leave it only by a change
//! that bumps the DOM version, which discards the numbers; a detached tree
//! can be freed, and its ids reused, without one.

use dasher_dom::{Document, NodeId};

use super::DomState;

/// `compareDocumentPosition` bits.
const DISCONNECTED: u32 = 0x01;
const PRECEDING: u32 = 0x02;
const FOLLOWING: u32 = 0x04;
const CONTAINS: u32 = 0x08;
const CONTAINED_BY: u32 = 0x10;
const IMPLEMENTATION_SPECIFIC: u32 = 0x20;

/// The numbering of the page's document, and the count that decides when to
/// make it.
#[derive(Default)]
pub(crate) struct Order {
    /// The DOM version `walked` and `spans` are for.
    version: u32,
    /// Steps walked to answer at this version, since the last numbering or
    /// the last look at whether to number.
    walked: usize,
    /// Whether `spans` holds the document as it is at this version.
    numbered: bool,
    /// By node (raw id): its place in the document in tree order, counted
    /// from 1, and the place of its last descendant (its own if it has
    /// none). Zeros for a node that is not in the document.
    spans: Vec<[u32; 2]>,
}

impl Order {
    /// Forgets what was learned about an earlier version of the tree.
    fn sync(&mut self, version: u32) {
        if self.version != version {
            self.version = version;
            self.walked = 0;
            self.numbered = false;
        }
    }

    /// `id`'s span, if the document is numbered and `id` is in it.
    fn span(&self, id: NodeId) -> Option<[u32; 2]> {
        if !self.numbered {
            return None;
        }
        self.spans
            .get(id.to_raw() as usize)
            .copied()
            .filter(|span| span[0] != 0)
    }

    /// Counts `steps` walked for `node`, and numbers the document if the
    /// walks have cost as much as that will, and `node` is in it.
    fn walked(&mut self, doc: &Document, node: NodeId, steps: usize) {
        self.walked += steps;
        if self.walked < doc.len() {
            return;
        }
        self.walked = 0;
        let root = doc.ancestors(node).last().unwrap_or(node);
        if root == doc.root() {
            self.number(doc);
        }
    }

    fn number(&mut self, doc: &Document) {
        self.spans.clear();
        self.spans.resize(doc.capacity() + 1, [0, 0]);
        let root = doc.root();
        let mut place = 0;
        let mut node = root;
        'tree: loop {
            place += 1;
            self.spans[node.to_raw() as usize][0] = place;
            if let Some(child) = doc.first_child(node) {
                node = child;
                continue;
            }
            // A leaf: it, and each ancestor it is the last node of, ends
            // here.
            loop {
                self.spans[node.to_raw() as usize][1] = place;
                if node == root {
                    break 'tree;
                }
                if let Some(sibling) = doc.next_sibling(node) {
                    node = sibling;
                    break;
                }
                node = doc.parent(node).expect("below the root");
            }
        }
        self.numbered = true;
    }
}

impl DomState {
    /// `a.compareDocumentPosition(b)`: where `b` is relative to `a`.
    pub(crate) fn compare_position(&self, doc: &Document, a: NodeId, b: NodeId) -> u32 {
        if a == b {
            return 0;
        }
        let mut order = self.order.borrow_mut();
        order.sync(self.version());
        if let (Some(span_a), Some(span_b)) = (order.span(a), order.span(b)) {
            return compare_spans(span_a, span_b);
        }
        let (position, steps) = compare_by_walking(doc, a, b);
        order.walked(doc, a, steps);
        position
    }

    /// Whether `id` is known, without a walk, to be in the page's document.
    pub(super) fn known_in_page_document(&self, id: NodeId) -> bool {
        let mut order = self.order.borrow_mut();
        order.sync(self.version());
        order.span(id).is_some()
    }

    /// Counts `steps` walked up from `node` to answer a question the
    /// document's numbering would have answered.
    pub(super) fn walked(&self, doc: &Document, node: NodeId, steps: usize) {
        let mut order = self.order.borrow_mut();
        order.sync(self.version());
        order.walked(doc, node, steps);
    }
}

/// Where the node spanning `b` is relative to the one spanning `a`, both in
/// the same tree and not the same node.
fn compare_spans(a: [u32; 2], b: [u32; 2]) -> u32 {
    if a[0] < b[0] {
        if b[0] <= a[1] {
            CONTAINED_BY | FOLLOWING
        } else {
            FOLLOWING
        }
    } else if a[0] <= b[1] {
        CONTAINS | PRECEDING
    } else {
        PRECEDING
    }
}

/// Where `b` is relative to `a` (not the same node), found by walking the
/// tree in place, and the number of steps that took. Nothing is allocated,
/// and no more of a child list is read than lies between the two nodes.
fn compare_by_walking(doc: &Document, a: NodeId, b: NodeId) -> (u32, usize) {
    // Bring the deeper node up to the other's depth, then both up together
    // until they meet or are siblings.
    let (depth_a, depth_b) = (doc.ancestors(a).count(), doc.ancestors(b).count());
    let mut steps = depth_a + depth_b;
    let nth_ancestor = |n: NodeId, levels: usize| match levels {
        0 => n,
        _ => doc.ancestors(n).nth(levels - 1).expect("within its depth"),
    };
    let mut x = nth_ancestor(a, depth_a.saturating_sub(depth_b));
    let mut y = nth_ancestor(b, depth_b.saturating_sub(depth_a));
    if x == y {
        let position = if depth_a < depth_b {
            CONTAINED_BY | FOLLOWING
        } else {
            CONTAINS | PRECEDING
        };
        return (position, steps);
    }
    loop {
        steps += 1;
        match (doc.parent(x), doc.parent(y)) {
            (Some(px), Some(py)) if px == py => break,
            (Some(px), Some(py)) => (x, y) = (px, py),
            // Two roots: the nodes are in different trees. Any order will
            // do, as long as it is consistent.
            _ => {
                let order = if a < b { FOLLOWING } else { PRECEDING };
                return (DISCONNECTED | IMPLEMENTATION_SPECIFIC | order, steps);
            }
        }
    }
    // Siblings: search forwards from both at once, so the cost is the
    // distance between them, not the length of the child list.
    let (mut after_x, mut after_y) = (x, y);
    let position = loop {
        steps += 2;
        match doc.next_sibling(after_x) {
            Some(n) if n == y => break FOLLOWING,
            Some(n) => after_x = n,
            None => break PRECEDING,
        }
        match doc.next_sibling(after_y) {
            Some(n) if n == x => break PRECEDING,
            Some(n) => after_y = n,
            None => break FOLLOWING,
        }
    };
    (position, steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition, slowly: ancestor chains from the root, compared.
    fn reference(doc: &Document, a: NodeId, b: NodeId) -> u32 {
        let chain = |n: NodeId| {
            let mut v: Vec<NodeId> = std::iter::once(n).chain(doc.ancestors(n)).collect();
            v.reverse();
            v
        };
        let (ca, cb) = (chain(a), chain(b));
        if ca[0] != cb[0] {
            let order = if a < b { FOLLOWING } else { PRECEDING };
            return DISCONNECTED | IMPLEMENTATION_SPECIFIC | order;
        }
        let common = ca.iter().zip(&cb).take_while(|(x, y)| x == y).count();
        if common == ca.len() {
            return CONTAINED_BY | FOLLOWING;
        }
        if common == cb.len() {
            return CONTAINS | PRECEDING;
        }
        let children: Vec<NodeId> =
            std::iter::successors(doc.first_child(ca[common - 1]), |&n| doc.next_sibling(n))
                .collect();
        let place = |n: NodeId| children.iter().position(|&c| c == n).unwrap();
        if place(ca[common]) < place(cb[common]) {
            FOLLOWING
        } else {
            PRECEDING
        }
    }

    /// A document of `count` elements of every shape (wide, deep, mixed),
    /// with a detached tree beside it; returns all the nodes.
    fn build(doc: &mut Document, count: usize, mut seed: u64) -> Vec<NodeId> {
        let mut random = move |below: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % below
        };
        let html = doc.create_element("html");
        doc.append_child(doc.root(), html).unwrap();
        let detached = doc.create_element("section");
        let mut nodes = vec![html, detached];
        for _ in 0..count {
            let node = doc.create_element("div");
            // Mostly under a recent node, so the tree gets deep too.
            let parent = match random(4) {
                0 => nodes[random(nodes.len())],
                _ => nodes[nodes.len() - 1 - random(nodes.len().min(3))],
            };
            match doc.first_child(parent) {
                Some(first) if random(3) == 0 => {
                    doc.insert_before(parent, node, Some(first)).unwrap();
                }
                _ => {
                    doc.append_child(parent, node).unwrap();
                }
            }
            nodes.push(node);
        }
        nodes.push(doc.root());
        nodes
    }

    #[test]
    fn walking_and_numbering_agree_with_the_definition() {
        for seed in 0..20 {
            let mut doc = Document::new();
            let nodes = build(&mut doc, 60, seed);
            let mut order = Order::default();
            order.number(&doc);
            for &a in &nodes {
                for &b in &nodes {
                    if a == b {
                        continue;
                    }
                    let expected = reference(&doc, a, b);
                    assert_eq!(compare_by_walking(&doc, a, b).0, expected, "{a:?} {b:?}");
                    if let (Some(sa), Some(sb)) = (order.span(a), order.span(b)) {
                        assert_eq!(compare_spans(sa, sb), expected, "{a:?} {b:?}");
                    }
                }
            }
            // Everything in the document is numbered, and nothing else.
            let in_document = |n: NodeId| doc.ancestors(n).last().unwrap_or(n) == doc.root();
            for &n in &nodes {
                assert_eq!(order.span(n).is_some(), in_document(n), "{n:?}");
            }
        }
    }

    #[test]
    fn numbers_only_once_walks_have_cost_as_much() {
        let mut doc = Document::new();
        let nodes = build(&mut doc, 200, 7);
        let (a, b) = (nodes[150], nodes[190]);
        let mut order = Order::default();
        order.sync(1);
        let (_, steps) = compare_by_walking(&doc, a, b);
        assert!(steps > 0 && steps < doc.len());
        order.walked(&doc, a, steps);
        assert!(order.span(a).is_none());
        order.walked(&doc, a, doc.len());
        assert!(order.span(a).is_some());
        // A change to the tree discards the numbers.
        order.sync(2);
        assert!(order.span(a).is_none());
        // Walks in a detached tree never number the document.
        order.walked(&doc, nodes[1], 10 * doc.len());
        assert!(order.span(nodes[1]).is_none());
        assert!(order.span(a).is_none());
    }
}
