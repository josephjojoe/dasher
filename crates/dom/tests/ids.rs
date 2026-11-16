//! Property test of the id index: after every random edit, every
//! `get_element_by_id` answer must equal a walk of the tree.
//!
//! The edits cover what changes ids or moves elements between tree scopes:
//! creating, cloning and releasing nodes, setting and removing `id` (and
//! attributes that only look like it), inserting and removing nodes,
//! `innerHTML`, shadow roots, template contents and extra documents.

use dasher_dom::{Document, Name, NodeId, NodeKind, ShadowRootInit};

/// A small, fixed-seed generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

/// Ids drawn from a small pool so they repeat, including case variants and
/// the empty id.
const IDS: &[&str] = &[
    "a",
    "b",
    "A",
    "c",
    "",
    "long-id-of-more-than-thirty-two-bytes",
];
const TAGS: &[&str] = &["div", "p", "span", "template", "section"];

/// Every live node the test knows about.
struct World {
    doc: Document,
    nodes: Vec<NodeId>,
    rng: Rng,
}

impl World {
    fn new(seed: u64, html: &str) -> Self {
        let doc = Document::parse(html);
        let nodes = all_nodes(&doc, doc.root());
        World {
            doc,
            nodes,
            rng: Rng(seed),
        }
    }

    fn any_node(&mut self) -> NodeId {
        let i = self.rng.below(self.nodes.len());
        self.nodes[i]
    }

    fn any_element(&mut self) -> Option<NodeId> {
        let elements: Vec<_> = self
            .nodes
            .iter()
            .copied()
            .filter(|&n| self.doc.is_element(n))
            .collect();
        (!elements.is_empty()).then(|| self.rng.pick(&elements))
    }

    fn adopt_new(&mut self, root: NodeId) {
        let new = all_nodes(&self.doc, root);
        self.nodes.extend(new);
        self.nodes.sort();
        self.nodes.dedup();
    }

    /// One random edit.
    fn edit(&mut self) {
        let doc = &mut self.doc;
        match self.rng.below(16) {
            0 | 1 => {
                let tag = self.rng.pick(TAGS);
                let el = doc.create_element(tag);
                if self.rng.below(2) == 0 {
                    let id = self.rng.pick(IDS);
                    doc.set_attribute(el, "id", id);
                }
                self.adopt_new(el);
            }
            2 | 3 => {
                if let Some(el) = self.any_element() {
                    let id = self.rng.pick(IDS);
                    match self.rng.below(3) {
                        0 => self.doc.set_attribute(el, "id", id),
                        1 => self.doc.set_attribute_named(el, Name::ID, id),
                        _ => self.doc.set_attribute_ns(el, "", "id", id),
                    }
                }
            }
            4 => {
                if let Some(el) = self.any_element() {
                    match self.rng.below(3) {
                        0 => {
                            self.doc.remove_attribute(el, "id");
                        }
                        1 => {
                            self.doc.remove_attribute_ns(el, Name::EMPTY, Name::ID);
                        }
                        // Not ids: a namespaced `id`, and `ID`.
                        _ => {
                            let id = self.rng.pick(IDS);
                            if self.rng.below(2) == 0 {
                                self.doc.set_attribute_ns(el, "urn:x", "x:id", id);
                            } else {
                                self.doc.set_attribute(el, "ID", id);
                            }
                        }
                    }
                }
            }
            5..=8 => {
                let parent = self.any_node();
                let child = self.any_node();
                let before = self.doc.first_child(parent);
                let before = if self.rng.below(2) == 0 { before } else { None };
                let _ = self.doc.insert_before(parent, child, before);
            }
            9 => {
                let n = self.any_node();
                self.doc.remove(n);
            }
            10 => {
                let n = self.any_node();
                if self.doc.kind(n) != NodeKind::Document {
                    let copy = self.doc.clone_node(n, self.rng.below(2) == 0);
                    self.adopt_new(copy);
                }
            }
            11 => {
                if let Some(el) = self.any_element() {
                    let (a, b) = (self.rng.pick(IDS), self.rng.pick(IDS));
                    let html = format!(
                        "<p id='{a}'><b id='{b}'></b></p><template><i id='{a}'></i></template><s id='{b}'>"
                    );
                    self.doc.set_inner_html(el, &html);
                    self.adopt_new(el);
                }
            }
            12 => {
                if let Some(el) = self.any_element()
                    && let Ok(root) = self.doc.attach_shadow(el, ShadowRootInit::open())
                {
                    let id = self.rng.pick(IDS);
                    self.doc.set_inner_html(el, "");
                    let inner = self.doc.create_element("div");
                    self.doc.set_attribute(inner, "id", id);
                    self.doc.append_child(root, inner).unwrap();
                    self.adopt_new(el);
                }
            }
            13 => {
                let d = self.doc.create_document();
                if self.rng.below(2) == 0 {
                    let id = self.rng.pick(IDS);
                    self.doc
                        .parse_document_into(d, &format!("<div id='{id}'><p id=a>"));
                }
                self.adopt_new(d);
            }
            _ => self.release_something(),
        }
    }

    /// Releases a random detached tree, if there is one that can be.
    fn release_something(&mut self) {
        let doc = &self.doc;
        let releasable: Vec<NodeId> = self
            .nodes
            .iter()
            .copied()
            .filter(|&n| {
                n != doc.root()
                    && doc.parent(n).is_none()
                    && doc.template_host(n).is_none()
                    && doc.shadow_host(n).is_none()
            })
            .collect();
        if releasable.is_empty() {
            return;
        }
        let n = self.rng.pick(&releasable);
        let gone = all_nodes(&self.doc, n);
        self.doc.release(n);
        self.nodes.retain(|x| !gone.contains(x));
    }

    /// Checks lookups against a walk, from every tree's root and from a few
    /// other nodes.
    fn check(&mut self, step: usize) {
        let tree_roots: Vec<NodeId> = self
            .nodes
            .iter()
            .copied()
            .filter(|&n| self.doc.parent(n).is_none())
            .collect();
        let mut roots = vec![self.doc.root()];
        for _ in 0..4 {
            roots.push(self.any_node());
            roots.push(self.rng.pick(&tree_roots));
        }
        let doc = &self.doc;
        for root in roots {
            for id in IDS.iter().copied().chain(["B", "missing"]) {
                let walked = doc
                    .descendants(root)
                    .find(|&n| doc.element_id(n) == Some(id))
                    .filter(|_| !id.is_empty());
                assert_eq!(
                    doc.get_element_by_id(root, id),
                    walked,
                    "step {step}: getElementById({id:?}) under {root:?}"
                );
                let mut all: Vec<_> = doc
                    .elements_with_id(id, true)
                    .filter(|&n| doc.is_descendant(n, root))
                    .collect();
                all.sort_by(|&a, &b| doc.compare_tree_order(a, b).unwrap());
                let walked: Vec<_> = doc
                    .descendants(root)
                    .filter(|&n| {
                        !id.is_empty()
                            && doc
                                .element_id(n)
                                .is_some_and(|v| v.eq_ignore_ascii_case(id))
                    })
                    .collect();
                assert_eq!(
                    all, walked,
                    "step {step}: elements_with_id({id:?}) under {root:?}"
                );
            }
        }
    }
}

/// `root` and everything under it, including template contents and
/// shadow trees: what releasing `root` frees.
fn all_nodes(doc: &Document, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        out.push(n);
        stack.extend(doc.children(n));
        stack.extend(doc.template_contents(n));
        stack.extend(doc.shadow_root(n));
    }
    out
}

#[test]
fn id_index_agrees_with_a_tree_walk() {
    for seed in 1..=30u64 {
        let mut world = World::new(
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            "<div id=a><p id=b></p><p id=A></p><template><b id=a></b></template></div><p id=a>",
        );
        for step in 0..300 {
            world.edit();
            // Keep the tree small enough that checking it all stays fast.
            if world.nodes.len() > 80 {
                world.release_something();
            }
            world.check(step);
        }
    }
}

#[test]
fn tree_order_comparison() {
    let doc = Document::parse("<div id=a><p id=b><i id=c></i></p><p id=d></p></div><p id=e>");
    let root = doc.root();
    let by = |id| doc.get_element_by_id(root, id).unwrap();
    let order = ["a", "b", "c", "d", "e"].map(by);
    for (i, &x) in order.iter().enumerate() {
        for (j, &y) in order.iter().enumerate() {
            assert_eq!(doc.compare_tree_order(x, y), Some(i.cmp(&j)), "{i} vs {j}");
        }
        assert_eq!(
            doc.compare_tree_order(root, x),
            Some(std::cmp::Ordering::Less)
        );
    }
    let mut other = Document::new();
    let x = other.create_element("div");
    let y = other.create_element("div");
    assert_eq!(other.compare_tree_order(x, y), None);
}

#[test]
fn shadow_roots_and_extra_documents_are_their_own_scopes() {
    let mut doc = Document::parse("<div id=host></div><p id=x>");
    let root = doc.root();
    let host = doc.get_element_by_id(root, "host").unwrap();
    let shadow = doc.attach_shadow(host, ShadowRootInit::open()).unwrap();
    let inner = doc.create_element("p");
    doc.set_attribute(inner, "id", "x");
    doc.append_child(shadow, inner).unwrap();
    let outer = doc.get_element_by_id(root, "x").unwrap();
    assert_ne!(outer, inner);
    assert_eq!(doc.get_element_by_id(shadow, "x"), Some(inner));

    let other = doc.create_document();
    doc.parse_document_into(other, "<p id=x>");
    let theirs = doc.get_element_by_id(other, "x").unwrap();
    assert!(theirs != inner && theirs != outer);
    assert_eq!(doc.get_element_by_id(root, "x"), Some(outer));

    // Moving the element between documents moves it between scopes.
    doc.remove(outer);
    assert_eq!(doc.get_element_by_id(root, "x"), None);
    let body = doc.body().unwrap();
    doc.append_child(body, theirs).unwrap();
    assert_eq!(doc.get_element_by_id(root, "x"), Some(theirs));
    assert_eq!(doc.get_element_by_id(other, "x"), None);
}
