//! DOM bindings: the page's [`Document`], the JS wrappers of its nodes and
//! the native functions `dom.js` builds the DOM API on.
//!
//! # Wrappers
//!
//! A node gets a JS wrapper the first time script sees it, and keeps the
//! same wrapper for as long as that wrapper is alive, so `a.firstChild ===
//! a.firstChild` and expando properties stick. The cache from node to
//! wrapper is weak: the wrapper's finalizer removes its entry.
//!
//! What keeps a wrapper alive is its *group*: all the wrappers of one tree
//! (a document, or a detached subtree) are linked in a ring of strong,
//! GC-traced references. So holding any wrapper of a tree keeps every other
//! wrapper of that tree (and its expandos and listeners) alive, as it must,
//! since script can walk to them. The document's ring is anchored by the
//! document wrapper, which the host holds, so wrappers of connected nodes
//! live as long as the page. A detached tree whose wrappers are all
//! unreachable is a garbage cycle that QuickJS collects.
//!
//! # Freeing nodes
//!
//! Nodes live in the document arena, which does not know about JS. When a
//! subtree is detached and nothing in it has a wrapper, or the last wrapper
//! of a detached tree is collected, the tree's root is queued as a
//! candidate. Candidates are released from the arena the next time the DOM
//! is mutated (or on [`JsRuntime::run_gc`]), after checking again that the
//! tree is still detached and unwrapped. Nodes are only ever released there,
//! which is what makes the node ids queued in between safe to look at.
//!
//! # Mutations
//!
//! Every change to the tree made from JS goes through [`DomState`]'s
//! mutation methods, which keep the wrapper groups in step with the trees
//! and bump the version that live collections use to notice changes. They
//! also report each change for `MutationObserver` (see the `mutation`
//! module) and count it as [`DomActivity`].

mod collection;
mod inner_text;
mod layout;
mod mutation;
mod natives;
mod wrapper;

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use dasher_css::SelectorList;
use dasher_dom::{Document, NodeId};
use rquickjs::{Ctx, Function, Object, qjs};

use crate::clock::Clock;
use crate::runtime::{JsRuntime, Shared};

pub(crate) use wrapper::{Node, wrap};

/// How many parsed selectors are kept for reuse.
const SELECTOR_CACHE_SIZE: usize = 64;

/// How busy scripts have been changing the document, for judging when a
/// page has settled. Read it with [`JsRuntime::dom_activity`].
///
/// Each change scripts make to a node in the document counts once: an
/// insertion or removal of children (a whole `innerHTML` or
/// `replaceChildren` counts once), an attribute set or removed, or new
/// text data. These are the changes `MutationObserver` reports, whether or
/// not anything observes them. Changes to detached nodes, such as a subtree
/// being built before it is inserted, do not count until it is inserted,
/// and neither do changes the host makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomActivity {
    /// How many changes scripts have made to the document so far.
    pub mutations: u64,
    /// The page time (see [`JsRuntime::now`]) of the latest one.
    pub last_mutation: Option<Duration>,
}

/// The page's document and the bookkeeping that ties it to JS.
pub(crate) struct DomState {
    doc: RefCell<Document>,
    /// Node -> its live wrapper object. Weak: entries are removed by the
    /// wrapper's finalizer. Never borrowed across anything that can run JS
    /// or the GC.
    wrappers: RefCell<HashMap<NodeId, qjs::JSValue>>,
    /// Nodes whose tree may have become garbage: roots of subtrees detached
    /// without wrappers, and nodes whose wrapper was collected.
    candidates: RefCell<Vec<NodeId>>,
    /// Bumped by every change to the tree or to attributes.
    version: Cell<u32>,
    selectors: RefCell<HashMap<String, Rc<SelectorList>>>,
    /// Nodes with registered mutation observers. Each has a wrapper, whose
    /// finalizer removes it.
    observed: RefCell<HashSet<NodeId>>,
    /// Mutation records waiting to be delivered to JS.
    pending: RefCell<Vec<mutation::Record>>,
    activity: Cell<DomActivity>,
    clock: Rc<Clock>,
    /// Set while the host holds node ids that must stay valid, such as the
    /// parser's open elements: garbage trees are kept until it is cleared.
    hold: Cell<bool>,
    /// Whether the layout hook is set (see the `layout` module).
    layout_watch: Cell<bool>,
    /// The version the layout hook last saw.
    layout_seen: Cell<u32>,
    /// The node limit every document of the page gets.
    node_limit: Option<usize>,
}

impl DomState {
    pub(crate) fn new(mut doc: Document, node_limit: Option<usize>, clock: Rc<Clock>) -> Self {
        doc.set_node_limit(node_limit);
        Self {
            node_limit,
            doc: RefCell::new(doc),
            wrappers: RefCell::default(),
            candidates: RefCell::default(),
            version: Cell::new(0),
            selectors: RefCell::default(),
            observed: RefCell::default(),
            pending: RefCell::default(),
            activity: Cell::default(),
            clock,
            hold: Cell::new(false),
            layout_watch: Cell::new(false),
            layout_seen: Cell::new(0),
        }
    }

    /// The document, for reading.
    pub(crate) fn doc(&self) -> Ref<'_, Document> {
        self.doc.borrow()
    }

    /// The document, for changing. Releases garbage trees first, so their
    /// arena slots can be reused.
    pub(crate) fn doc_mut(&self) -> RefMut<'_, Document> {
        let mut doc = self.doc.borrow_mut();
        self.sweep(&mut doc);
        doc
    }

    pub(crate) fn node_limit(&self) -> Option<usize> {
        self.node_limit
    }

    /// Whether the document has gone over its node limit. Called from the
    /// interrupt handler, so it must not panic if the document is borrowed;
    /// it then reports no (the next check will see it).
    pub(crate) fn node_limit_exceeded(&self) -> bool {
        self.doc
            .try_borrow()
            .is_ok_and(|doc| doc.node_limit_exceeded())
    }

    pub(crate) fn version(&self) -> u32 {
        self.version.get()
    }

    pub(crate) fn bump(&self) {
        self.version.set(self.version.get().wrapping_add(1));
    }

    /// A parsed selector list, from the cache if it was seen recently.
    pub(crate) fn selector(
        &self,
        text: &str,
    ) -> Result<Rc<SelectorList>, dasher_css::SelectorError> {
        if let Some(list) = self.selectors.borrow().get(text) {
            return Ok(Rc::clone(list));
        }
        let list = Rc::new(SelectorList::parse(text)?);
        let mut cache = self.selectors.borrow_mut();
        if cache.len() >= SELECTOR_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(text.to_owned(), Rc::clone(&list));
        Ok(list)
    }

    /// Queues `id`'s tree to be released if it turns out to be garbage.
    pub(crate) fn add_candidate(&self, id: NodeId) {
        self.candidates.borrow_mut().push(id);
    }

    /// Releases the trees of queued candidates that are detached and have
    /// no wrapper left. Waits while mutation records, which may name nodes
    /// of those trees, are queued, and while the host holds detached nodes.
    fn sweep(&self, doc: &mut Document) {
        if self.hold.get() || self.has_pending_records() {
            return;
        }
        let candidates = std::mem::take(&mut *self.candidates.borrow_mut());
        if candidates.is_empty() {
            return;
        }
        // Every candidate is still allocated here: nodes are only released
        // below, and a tree is only released once nothing refers to it. Find
        // all the roots before releasing any of them.
        let mut roots: Vec<NodeId> = candidates
            .into_iter()
            .map(|n| group_root(doc, n))
            .filter(|&r| r != doc.root())
            .collect();
        roots.sort_unstable();
        roots.dedup();
        let garbage: Vec<NodeId> = {
            let wrappers = self.wrappers.borrow();
            roots
                .into_iter()
                .filter(|&r| !any_in_tree(doc, r, |n| wrappers.contains_key(&n)))
                .collect()
        };
        for root in garbage {
            doc.release(root);
        }
    }
}

/// The root of `id`'s tree, looking through template contents to their
/// template, since a template keeps its contents alive.
pub(crate) fn group_root(doc: &Document, mut id: NodeId) -> NodeId {
    loop {
        match doc.parent(id).or_else(|| doc.template_host(id)) {
            Some(up) => id = up,
            None => return id,
        }
    }
}

/// Whether `f` holds for `root` or any node under it, including nodes in
/// template contents. Skips the subtrees rooted at nodes in `skip`.
pub(crate) fn any_in_tree_except(
    doc: &Document,
    root: NodeId,
    skip: &[NodeId],
    mut f: impl FnMut(NodeId) -> bool,
) -> bool {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if skip.contains(&n) {
            continue;
        }
        if f(n) {
            return true;
        }
        stack.extend(doc.children(n));
        stack.extend(doc.template_contents(n));
    }
    false
}

fn any_in_tree(doc: &Document, root: NodeId, f: impl FnMut(NodeId) -> bool) -> bool {
    any_in_tree_except(doc, root, &[], f)
}

/// Installs the DOM's native functions as `native.dom`.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> rquickjs::Result<()> {
    let dom = Object::new(ctx.clone())?;
    natives::install(&dom, &shared.dom)?;
    let s = Rc::clone(shared);
    dom.set(
        "cookie",
        Function::new(ctx.clone(), move || {
            let url = s.window.url();
            s.net.cookies().script_cookies_for(&url)
        })?,
    )?;
    let s = Rc::clone(shared);
    dom.set(
        "setCookie",
        Function::new(ctx.clone(), move |cookie: String| {
            let url = s.window.url();
            s.net.cookies().set_from_script(&url, &cookie);
        })?,
    )?;
    native.set("dom", dom)
}

impl JsRuntime {
    /// How much scripts have changed the document so far, and when they
    /// last did, for telling when a page has gone quiet.
    pub fn dom_activity(&self) -> DomActivity {
        self.shared().dom.activity.get()
    }

    /// Runs `f` with read access to the page's document.
    ///
    /// # Panics
    ///
    /// If called from inside JS, e.g. from a host function.
    pub fn with_document<R>(&self, f: impl FnOnce(&Document) -> R) -> R {
        f(&self.shared().dom.doc())
    }

    /// Runs `f` with write access to the page's document, for host-side
    /// changes such as the page loader marking scripts as started.
    ///
    /// Afterwards the node wrappers are re-grouped to match the tree, so `f`
    /// may move nodes that scripts hold. It must not
    /// [release](Document::release) nodes; detached nodes are freed
    /// automatically once nothing refers to them.
    pub fn with_document_mut<R>(&self, f: impl FnOnce(&mut Document) -> R) -> R {
        let dom = &self.shared().dom;
        let result = f(&mut dom.doc_mut());
        dom.bump();
        self.with(|ctx| wrapper::regroup_all(&ctx, dom));
        result
    }

    /// While `hold` is set, nodes detached from the document are not freed,
    /// even once nothing refers to them, so node ids the host keeps across
    /// script runs stay valid. The page loader holds them while it parses,
    /// since the parser keeps ids of open elements that scripts may remove.
    /// Clearing it lets the garbage be freed again.
    pub fn hold_detached_nodes(&self, hold: bool) {
        self.shared().dom.hold.set(hold);
    }

    /// Replaces the page's document, e.g. with one parsed from the response.
    ///
    /// `document` keeps its identity and now reflects the new tree. Wrappers
    /// of nodes from the old document stay usable as objects, but DOM
    /// operations on them throw an `InvalidStateError`. Call this before
    /// running the page's scripts.
    pub fn set_document(&self, mut document: Document) {
        let dom = &self.shared().dom;
        document.set_node_limit(dom.node_limit);
        self.with(|ctx| wrapper::detach_all(&ctx, dom));
        let old = std::mem::replace(&mut *dom.doc.borrow_mut(), document);
        dom.candidates.borrow_mut().clear();
        dom.selectors.borrow_mut().clear();
        // The document node keeps its wrapper, and so its observers.
        let root = dom.doc.borrow().root();
        dom.observed.borrow_mut().retain(|&n| n == root);
        dom.pending.borrow_mut().clear();
        dom.bump();
        drop(old);
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::{ClockMode, JsRuntime, RuntimeOptions};

    use super::wrapper::check_groups;

    fn check(js: &JsRuntime, step: &str) {
        let dom = Rc::clone(&js.shared().dom);
        js.with(|ctx| check_groups(&ctx, &dom))
            .unwrap_or_else(|e| panic!("after {step}: {e}"));
    }

    /// Random mutations from script, checking after each that the wrapper
    /// rings still match the trees and that freed nodes are never touched.
    #[test]
    fn rings_follow_random_mutations() {
        let js = JsRuntime::new(RuntimeOptions {
            clock: ClockMode::Manual,
            ..RuntimeOptions::default()
        })
        .unwrap();
        js.run_script(
            r#"
            let seed = 7;
            const rand = (n) => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed % n; };
            const pool = [document.body];
            for (let i = 0; i < 12; i++) pool.push(document.createElement(i % 3 ? 'div' : 'span'));
            pool.push(document.createDocumentFragment(), document.createDocumentFragment());
            const template = document.createElement('template');
            pool.push(template, template.content);
            const pick = () => pool[rand(pool.length)];
            globalThis.step = () => {
              const a = pick(), b = pick();
              const op = rand(11);
              try {
                switch (op) {
                  case 0: a.appendChild(b); break;
                  case 1: a.insertBefore(b, a.firstChild); break;
                  case 2: if (b.parentNode) b.remove(); break;
                  case 3: a.innerHTML = '<i><b></b></i><u></u>'; break;
                  case 4: if (b.parentNode) b.parentNode.replaceChild(a, b); break;
                  case 5: { const c = a.firstChild; if (c) { pool.push(c); c.k = 1; } break; }
                  case 6: { const c = a.querySelector('*'); if (c) pool.push(c); break; }
                  case 7: a.textContent = 'x'; break;
                  case 8: a.replaceChildren(b, 'y'); break;
                  case 9: if (pool.length > 10) pool.splice(rand(pool.length - 1) + 1, 1); break;
                  case 10: a.append(b.cloneNode(true)); break;
                }
              } catch (e) {
                if (!(e instanceof DOMException) && !(e instanceof TypeError)) throw e;
              }
              return op;
            };
            "#,
            "setup.js",
        )
        .unwrap();
        check(&js, "setup");
        for i in 0..1500 {
            let op = js.eval_json("step()").unwrap().unwrap();
            check(&js, &format!("step {i} (op {op})"));
            if i % 50 == 0 {
                js.run_gc();
                check(&js, &format!("gc after step {i}"));
            }
        }
    }
}
