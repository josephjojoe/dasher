//! DOM bindings: the page's [`Document`], the JS wrappers of its nodes and
//! the native functions `dom.js` builds the DOM API on.
//!
//! # Wrappers
//!
//! A node gets a JS wrapper the first time script sees it, and keeps the
//! same wrapper for as long as that wrapper is alive, so `a.firstChild ===
//! a.firstChild` and expando properties stick. The cache from node to
//! wrapper is weak: the wrapper's finalizer removes its entry. It is a table
//! of object pointers indexed by the node's arena slot (`wrapper::Wrappers`),
//! so finding a node's wrapper is an index and a null check. A slot is only
//! reused once its node is released, which needs it to have no wrapper.
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
//! candidate. So is the tree a node leaves behind when it is removed or
//! moved out of a detached tree, since the node may have held that tree's
//! only wrapper. Candidates are released from the arena the next time the
//! DOM is mutated (or on [`JsRuntime::run_gc`]), after checking again that
//! the tree is still detached and unwrapped. Nodes are only ever released
//! there, which is what makes the node ids queued in between safe to look
//! at.
//!
//! Wrappers in rings are cycles, which only QuickJS's cycle collector
//! frees, and it runs when the JS heap has grown, which says little about
//! the arena: a loop making wrapped nodes and dropping them could reach the
//! node limit first. So the DOM runs the collector itself when the arena
//! has grown well past what the last collection left (see [`next_gc`]),
//! before it nears the limit.
//!
//! The host holds node ids too: the page loader's parser keeps its open
//! elements (taken from it at every [`JsRuntime::with_parser`] call), and
//! the loader keeps script elements waiting to run. Those are *held* (see
//! [`JsRuntime::hold_node`]), and a tree with a held node is kept like one
//! with a wrapper; when the last hold on a node goes, its tree is queued as
//! a candidate again. Holding only what is used, rather than everything
//! while the page parses, is what lets a script that churns the DOM while
//! the parser is still running stay within its node limit.
//!
//! # Events
//!
//! A node's event listeners live in its wrapper (`NodeWrapper::listeners`),
//! which reports them to the GC with its ring links, so they live exactly
//! as long as the wrapper group. Listeners, `Event` objects and dispatch
//! are in the `events` module, which walks event paths on node ids and
//! only wraps the nodes of a path once a listener could hear the event.
//! Event targets that are not nodes use the same native members, so the
//! pattern for them is in `events/natives.rs`.
//!
//! # Mutations
//!
//! Every change to the tree made from JS goes through [`DomState`]'s
//! mutation methods, which keep the wrapper groups in step with the trees
//! and bump the version that live collections use to notice changes. They
//! also report each change for `MutationObserver` (see the `mutation`
//! module) and count it as [`DomActivity`].
//!
//! # Native members
//!
//! Most of the DOM API is JS classes in `dom.js` whose members call the
//! natives in `native.dom` (the `natives` module): two calls through the
//! interpreter and rquickjs for every DOM call. The members pages call most
//! are instead native functions sitting directly on the prototypes, as in a
//! browser: reading `el.tagName` calls Rust straight from the property
//! lookup. They are listed in `MEMBERS` in the `members` module; these are
//! the steps for adding one.
//!
//! 1. In `dom.js`, keep (or write) the member where it belongs, a class
//!    getter or method or a mixin member, with `return nativeMember();` as
//!    its body. The stand-in gives the property its place on the prototype
//!    and its attributes, which the native keeps (and which exposing the
//!    interface then makes enumerable, like every member; see
//!    `interfaceMembers` in `prelude.js`). An accessor's setter stays in JS
//!    unless a `setter` entry replaces it (its stand-in body is then
//!    `nativeMember();`).
//! 2. In `members.rs`, write the body: a plain `fn(&Call, NodeId, &[JSValue])
//!    -> Outcome` that reads the document through `Call::doc` and says what
//!    to return as a `Ret` (a node, a cached name, a string, a number...)
//!    or what to throw as a `Thrown` (or a `Write` body; see below). Add a
//!    `getter(...)`, `setter(...)`, `method(...)`, `reader(...)` or
//!    `writer(...)` entry to `MEMBERS` with the property name, the `Receiver` the
//!    interface needs (`this` must be a node of that kind, or the call
//!    throws `TypeError: Illegal invocation` as browsers do) and the keys of
//!    the prototypes it goes on, from the `interfaces` object `dom.js`
//!    passes to `N.init` (add a prototype there if needed).
//! 3. `N.init` replaces each stand-in with the native and fails if one is
//!    missing; the `dom_members` tests check that no stand-in is left, and
//!    the descriptors, names, lengths and `toString` of the natives.
//!
//! What a call costs: QuickJS calls `members::call_member`, the one C
//! function behind every member (the entry's index is its magic number). It
//! finds the `NodeWrapper` behind `this` from the object's opaque pointer
//! and rquickjs's class id and vtable, which it learns from the document
//! wrapper (`Members`, reached through the context's opaque pointer; see
//! the `native_data` module for what else hangs off it), so
//! there is no conversion, closure, `Rc` clone or hash lookup. Nodes come
//! back through the wrapper table, and names (tag names, local names,
//! namespaces) as atoms cached by name id, so the same string is never
//! converted from UTF-8 twice. Only a node without a wrapper yet, a
//! one-off string or an error goes through rquickjs.
//!
//! Two kinds of body:
//!
//! - `Body::Read` (the `getter`, `method` and `reader` entries) runs with
//!   the document borrowed (shared) for the whole call, so it must not
//!   change the document or run JS. Its arguments are converted first, as
//!   the entry's `Arg`s say (`Str`: objects and symbols go through
//!   `ToString`; `Uint32`: `ToUint32`), since converting an object can run
//!   page script; the body reads them with `Call::str_arg` and
//!   `Call::u32_arg`.
//! - `Body::Write` (`setter`, `writer`) runs with nothing borrowed: a
//!   `fn(&Write) -> Result<Ret, Thrown>` that borrows the document as it
//!   needs to and may run JS: converting arguments (`Write::str_arg`),
//!   setting attributes through `natives::set_attr` (which reports
//!   mutation records and custom element reactions like `setAttribute`).
//!
//! Strings come in as UTF-8 from QuickJS (`JsStr`), lone surrogates turned
//! into U+FFFD as `internals.usv` does. A `DOMException`
//! (`Thrown::Dom`) is thrown once the document is let go, since making
//! one runs JS. Besides node receivers there are the host objects of the
//! `host` module (`Receiver::Style`, `Receiver::Tokens`: `this` is the
//! object, the node its element) and plain functions (`Kind::Function`,
//! `Receiver::ElementArg`: `getComputedStyle`, put on the `functions`
//! object rather than a prototype). The natives `dom.js` calls internally
//! (`N.nav` and friends) stay, since glue code must not see page script's
//! overrides of the public members; where `dom.js` builds on a native
//! member it calls it as `N.init` left it (the `natives` object in
//! `dom.js`).
//!
//! A member made with `Member::or_js` keeps the function `dom.js` defines in
//! its place as its *fallback*: the stand-in is then the full JS
//! implementation, not `nativeMember()`. Calls with fewer arguments than
//! the member requires go straight to it (so the glue's own "1 argument
//! required" message is thrown), and a body hands it anything it does not
//! handle by returning `Thrown::Fallback`, before it has done anything
//! scripts could notice: an argument that is not a node, an event handler
//! attribute (whose changes the glue tracks), a script or custom element to
//! create, an extra document. That keeps every rare case exactly as it was
//! while the common path runs no JS. The fallbacks, and the glue's hooks the
//! natives call (`connected()` for the scripts an insertion connects, the
//! base URL), are kept in the `hooks` module.
//!
//! The live lists of the `lists` module (`childNodes`, `children`,
//! `getElementsBy*`, `attributes`) are exotic QuickJS objects like the
//! host objects, answering their indexed and named properties without a
//! `Proxy`; `Receiver::List` members (`length`, `item`...) get the list
//! object as the call's `node`. The IDL attributes HTML elements reflect
//! (`a.href`, `el.hidden`...) are made native from the list `reflect()` in
//! `dom.js` builds (see the `attrs` module).

mod attrs;
mod collection;
mod custom;
mod edits;
mod hooks;
mod host;
mod inner_text;
mod layout;
mod lists;
mod members;
mod mutation;
mod natives;
mod order;
mod queries;
mod shadow;
mod style;
mod wrapper;

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use dasher_css::SelectorList;
use dasher_dom::{Document, DocumentParser, NodeId, NodeKind};
use rquickjs::{Ctx, Function, Object, qjs};

use crate::clock::Clock;
use crate::runtime::{JsRuntime, Shared};

pub(crate) use layout::Geometry;
pub(crate) use members::Members;
pub(crate) use wrapper::{Node, NodeWrapper, wrap};

/// How many parsed selectors each generation of the selector cache holds.
/// See [`SelectorCache`].
const SELECTOR_CACHE_SIZE: usize = 128;

/// Parsed selectors by text, so a page that runs the same query over and
/// over parses it once.
///
/// Two generations make it roughly least-recently-used without the
/// bookkeeping: new entries go into `current`; when it is full it becomes
/// `previous` (dropping the old `previous`), and a hit in `previous` moves
/// the entry back to `current`. A burst of one-off selectors (such as
/// `'#' + id` for every element) therefore cannot flush the few a page
/// uses all the time. At most `2 * SELECTOR_CACHE_SIZE` lists are kept,
/// a few tens of KiB.
#[derive(Default)]
struct SelectorCache {
    current: HashMap<String, Rc<SelectorList>>,
    previous: HashMap<String, Rc<SelectorList>>,
}

impl SelectorCache {
    fn get(&mut self, text: &str) -> Option<Rc<SelectorList>> {
        if let Some(list) = self.current.get(text) {
            return Some(Rc::clone(list));
        }
        let (text, list) = self.previous.remove_entry(text)?;
        self.insert(text, Rc::clone(&list));
        Some(list)
    }

    fn insert(&mut self, text: String, list: Rc<SelectorList>) {
        if self.current.len() >= SELECTOR_CACHE_SIZE {
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(text, list);
    }

    fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
    }
}

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
    /// wrapper's finalizer.
    wrappers: wrapper::Wrappers,
    /// Nodes whose tree may have become garbage: roots of subtrees detached
    /// without wrappers, and nodes whose wrapper was collected.
    candidates: RefCell<Vec<NodeId>>,
    /// Bumped by every change to the tree or to attributes.
    version: Cell<u32>,
    selectors: RefCell<SelectorCache>,
    /// Nodes with registered mutation observers. Each has a wrapper, whose
    /// finalizer removes it.
    observed: RefCell<HashSet<NodeId>>,
    /// Mutation records waiting to be delivered to JS.
    pending: RefCell<Vec<mutation::Record>>,
    /// The names custom elements are defined for (see the `custom` module).
    ce_names: RefCell<HashSet<String>>,
    /// Custom elements. Each has a wrapper, whose finalizer removes it.
    ce_custom: RefCell<HashSet<NodeId>>,
    /// Custom element reactions waiting to be delivered to JS.
    ce_reactions: RefCell<Vec<custom::Reaction>>,
    activity: Cell<DomActivity>,
    clock: Rc<Clock>,
    /// Nodes the host holds (see [`JsRuntime::hold_node`]), with how many
    /// holds each has. A tree with a held node is not released.
    held: RefCell<HashMap<NodeId, u32>>,
    /// Reflected attributes described before `N.init` (see `attrs::reflect`).
    reflected: RefCell<Vec<attrs::Reflected>>,
    /// The nodes held for the parser (see [`JsRuntime::with_parser`]).
    parser_held: RefCell<Vec<NodeId>>,
    /// Bumped by [`JsRuntime::set_document`], so holds taken on the old
    /// document's nodes are not let go of in the new one.
    generation: Cell<u64>,
    /// The JS runtime, to run its cycle collector; null before the DOM is
    /// installed and once the runtime is going away.
    rt: Cell<*mut qjs::JSRuntime>,
    /// The number of live nodes at which to run the cycle collector next.
    gc_at: Cell<usize>,
    /// Whether the layout hook is set (see the `layout` module).
    layout_watch: Cell<bool>,
    /// The version the layout hook last saw.
    layout_seen: Cell<u32>,
    /// The node limit every document of the page gets.
    node_limit: Option<usize>,
    /// The viewport and visibility policy the synthetic layout follows.
    geometry: Geometry,
    layout: layout::LayoutCache,
    /// Whether scripts were told the parser inserted the body.
    body_seen: Cell<bool>,
    /// Set once script has made a document besides the page's.
    extra_documents: Cell<bool>,
    /// The node documents of detached trees that belong to a document other
    /// than the page's: tree root -> that document node. A tree under a
    /// document node belongs to it, and any other tree to the page. Empty
    /// unless script uses extra documents.
    owners: RefCell<HashMap<NodeId, NodeId>>,
    /// The page's document in tree order, once enough is asked about it.
    order: RefCell<order::Order>,
}

impl DomState {
    pub(crate) fn new(
        mut doc: Document,
        node_limit: Option<usize>,
        clock: Rc<Clock>,
        geometry: Geometry,
    ) -> Self {
        doc.set_node_limit(node_limit);
        Self {
            node_limit,
            geometry,
            layout: layout::LayoutCache::default(),
            doc: RefCell::new(doc),
            wrappers: wrapper::Wrappers::default(),
            candidates: RefCell::default(),
            version: Cell::new(0),
            selectors: RefCell::default(),
            observed: RefCell::default(),
            pending: RefCell::default(),
            ce_names: RefCell::default(),
            ce_custom: RefCell::default(),
            ce_reactions: RefCell::default(),
            activity: Cell::default(),
            clock,
            held: RefCell::default(),
            reflected: RefCell::default(),
            parser_held: RefCell::default(),
            generation: Cell::new(0),
            rt: Cell::new(std::ptr::null_mut()),
            gc_at: Cell::new(next_gc(1, node_limit)),
            layout_watch: Cell::new(false),
            layout_seen: Cell::new(0),
            body_seen: Cell::new(false),
            extra_documents: Cell::new(false),
            owners: RefCell::default(),
            order: RefCell::default(),
        }
    }

    /// The document, for reading.
    pub(crate) fn doc(&self) -> Ref<'_, Document> {
        self.doc.borrow()
    }

    /// The document, for changing. Releases garbage trees first, so their
    /// arena slots can be reused, after collecting garbage wrappers if the
    /// arena has grown enough to call for it.
    ///
    /// Like anything that can allocate JS values, this may run the GC, so
    /// it must not be called while `wrappers` or the other cells finalizers
    /// use are borrowed.
    pub(crate) fn doc_mut(&self) -> RefMut<'_, Document> {
        let collect = self.doc.borrow().len() >= self.gc_at.get();
        let rt = self.rt.get();
        if collect && !rt.is_null() {
            // SAFETY: `rt` is the live runtime this DOM belongs to (it is
            // cleared before the runtime is freed), used on its own thread.
            // Collecting runs only finalizers, which do not run JS and touch
            // none of what is borrowed here.
            unsafe { qjs::JS_RunGC(rt) };
        }
        let mut doc = self.doc.borrow_mut();
        self.sweep(&mut doc);
        if collect {
            self.gc_at.set(next_gc(doc.len(), self.node_limit));
        }
        doc
    }

    /// Sets the runtime whose cycle collector [`doc_mut`](Self::doc_mut)
    /// runs, or clears it with null.
    pub(crate) fn set_runtime(&self, rt: *mut qjs::JSRuntime) {
        self.rt.set(rt);
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
        if let Some(list) = self.selectors.borrow_mut().get(text) {
            return Ok(list);
        }
        let list = Rc::new(SelectorList::parse(text)?);
        self.selectors
            .borrow_mut()
            .insert(text.to_owned(), Rc::clone(&list));
        Ok(list)
    }

    /// Notes that script made a document besides the page's.
    pub(crate) fn note_extra_document(&self) {
        self.extra_documents.set(true);
    }

    /// `id`'s node document: the document node at the root of its tree, or
    /// for a detached tree the document it was made in or last adopted by.
    pub(crate) fn owner_document(&self, doc: &Document, id: NodeId) -> NodeId {
        if !self.extra_documents.get() {
            return doc.root();
        }
        if self.known_in_page_document(id) {
            return doc.root();
        }
        let (mut root, mut steps) = (id, 0);
        while let Some(up) = doc.parent(root).or_else(|| owner(doc, root)) {
            root = up;
            steps += 1;
        }
        self.walked(doc, id, steps);
        if doc.kind(root) == NodeKind::Document {
            return root;
        }
        self.owners
            .borrow()
            .get(&root)
            .copied()
            .unwrap_or(doc.root())
    }

    /// Makes `owner` the node document of the detached tree rooted at `root`.
    pub(crate) fn set_owner(&self, doc: &Document, root: NodeId, owner: NodeId) {
        if doc.parent(root).is_some() || self::owner(doc, root).is_some() {
            return;
        }
        let mut owners = self.owners.borrow_mut();
        if owner == doc.root() || doc.kind(root) == NodeKind::Document {
            owners.remove(&root);
        } else {
            owners.insert(root, owner);
        }
    }

    /// Keeps node documents in step after the tree at `root` was taken out
    /// of `from`: it still belongs to the document it was in.
    pub(crate) fn removed_from(&self, doc: &Document, root: NodeId, from: NodeId) {
        if self.extra_documents.get() {
            self.set_owner(doc, root, self.owner_document(doc, from));
        }
    }

    /// Keeps node documents in step after `node` was inserted: it now
    /// belongs to the document of its new tree.
    pub(crate) fn inserted(&self, doc: &Document, node: NodeId) {
        if self.extra_documents.get() && doc.parent(node).is_some() {
            self.owners.borrow_mut().remove(&node);
        }
    }

    /// Queues `id`'s tree to be released if it turns out to be garbage.
    pub(crate) fn add_candidate(&self, id: NodeId) {
        self.candidates.borrow_mut().push(id);
    }

    /// Adds a hold on `id` (see [`JsRuntime::hold_node`]).
    fn hold(&self, id: NodeId) {
        *self.held.borrow_mut().entry(id).or_default() += 1;
    }

    /// Takes away a hold on `id`. Once it has none, its tree may be garbage.
    fn unhold(&self, id: NodeId) {
        let mut held = self.held.borrow_mut();
        let Some(count) = held.get_mut(&id) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            held.remove(&id);
            drop(held);
            self.add_candidate(id);
        }
    }

    /// Makes `nodes` the ones held for the parser, letting go of those held
    /// for it before.
    fn hold_for_parser(&self, nodes: Vec<NodeId>) {
        for &n in &nodes {
            self.hold(n);
        }
        let old = std::mem::replace(&mut *self.parser_held.borrow_mut(), nodes);
        for n in old {
            self.unhold(n);
        }
    }

    /// Queues the tree `from` is in as a candidate if it is detached: a
    /// node was just taken out of it, which may have held the tree's only
    /// wrapper. Nothing to do if `from` has one itself, as it usually does
    /// (script called a method on it).
    pub(crate) fn left_behind(&self, doc: &Document, from: NodeId) {
        if self.wrappers.contains(from) {
            return;
        }
        if group_root(doc, from) != doc.root() {
            self.add_candidate(from);
        }
    }

    /// Releases the trees of queued candidates that are detached and have
    /// no wrapper or held node left. Waits while mutation records or custom
    /// element reactions, which may name nodes of those trees, are queued
    /// (they are only queued for the length of one DOM call, until they are
    /// delivered to JS, which wraps the nodes they name).
    fn sweep(&self, doc: &mut Document) {
        if self.has_pending_records() || self.has_pending_reactions() {
            return;
        }
        let candidates = std::mem::take(&mut *self.candidates.borrow_mut());
        if candidates.is_empty() {
            return;
        }
        // Every candidate is still allocated here, but for those the parser
        // held and released itself when it finished: nodes are otherwise
        // only released below, and a tree is only released once nothing
        // refers to it. Find all the roots before releasing any of them.
        let mut roots: Vec<NodeId> = candidates
            .into_iter()
            .filter(|&n| !doc.is_released(n))
            .map(|n| group_root(doc, n))
            .filter(|&r| r != doc.root())
            .collect();
        roots.sort_unstable();
        roots.dedup();
        let garbage: Vec<NodeId> = {
            let wrappers = &self.wrappers;
            let held = self.held.borrow();
            roots
                .into_iter()
                .filter(|&r| {
                    !any_in_tree(doc, r, |n| {
                        wrappers.contains(n) || (!held.is_empty() && held.contains_key(&n))
                    })
                })
                .collect()
        };
        if !garbage.is_empty() && !self.owners.borrow().is_empty() {
            // Forget released trees, and the documents that owned them.
            self.owners.borrow_mut().retain(|&tree, &mut owner| {
                !garbage.contains(&group_root(doc, tree)) && !garbage.contains(&owner)
            });
        }
        for root in garbage {
            doc.release(root);
        }
    }
}

/// The live node count at which to run the cycle collector again, after
/// one left `live` nodes: twice as many and then some, so collections stay
/// rare relative to the nodes made between them, but short of the node
/// limit, and at least an eighth more, so a page with nearly its limit in
/// live nodes does not collect on every change.
fn next_gc(live: usize, limit: Option<usize>) -> usize {
    const SLACK: usize = 4096;
    let doubled = live.saturating_mul(2).saturating_add(SLACK);
    let near_limit = limit.map_or(usize::MAX, |l| l - l / 8);
    doubled.min(near_limit).max(live + live / 8 + 1024)
}

/// A hold on a node of the page's document, from [`JsRuntime::hold_node`]:
/// the node is not freed until this is dropped. It does nothing once the
/// page's document has been [replaced](JsRuntime::set_document).
pub struct HeldNode {
    dom: Rc<DomState>,
    node: NodeId,
    generation: u64,
}

impl HeldNode {
    pub(crate) fn new(dom: &Rc<DomState>, node: NodeId) -> Self {
        dom.hold(node);
        Self {
            dom: Rc::clone(dom),
            node,
            generation: dom.generation.get(),
        }
    }

    /// The node held.
    pub fn node(&self) -> NodeId {
        self.node
    }
}

impl std::fmt::Debug for HeldNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HeldNode").field(&self.node).finish()
    }
}

impl Drop for HeldNode {
    fn drop(&mut self) {
        if self.dom.generation.get() == self.generation {
            self.dom.unhold(self.node);
        }
    }
}

/// The node that keeps `id`'s parentless node alive besides its own wrapper:
/// a template keeps its contents, and a host its shadow root.
pub(crate) fn owner(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.template_host(id).or_else(|| doc.shadow_host(id))
}

/// The root of `id`'s tree, looking through template contents to their
/// template and shadow roots to their host, since those keep them alive.
pub(crate) fn group_root(doc: &Document, mut id: NodeId) -> NodeId {
    loop {
        match doc.parent(id).or_else(|| owner(doc, id)) {
            Some(up) => id = up,
            None => return id,
        }
    }
}

/// Whether `f` holds for `root` or any node under it, including nodes in
/// template contents and shadow trees. Skips the subtrees rooted at nodes
/// in `skip`.
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
        stack.extend(doc.shadow_root(n));
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
    // SAFETY: `ctx` is a live context.
    shared
        .dom
        .set_runtime(unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) });
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

    /// Runs `f`, which parses into the page's document with `parser`, with
    /// write access to the document as [`with_document_mut`] gives.
    ///
    /// Afterwards the nodes the parser still refers to (its
    /// [`held_nodes`](DocumentParser::held_nodes)) are held, as
    /// [`hold_node`](Self::hold_node) holds them, in place of those held
    /// after the previous call; so the page's one parser must be driven
    /// only through this. Once it has [finished](DocumentParser::finish),
    /// it holds nothing. Nodes scripts detach that it does not refer to are
    /// freed as usual, while parsing goes on.
    ///
    /// Unlike other host changes, what the parser builds is reported to
    /// `MutationObserver`s, as browsers report it: a `childList` record for
    /// each node inserted (or moved, for misnested markup), a
    /// `characterData` record when text is added to a text node that was
    /// there before this call, and an `attributes` record for each
    /// attribute a repeated `<html>` or `<body>` tag adds. If any are
    /// queued, a microtask checkpoint follows, as one does in browsers
    /// before the parser runs a script and after it parses each chunk of
    /// input, so observers have seen the new nodes before the next script
    /// runs. While nothing is observed the parser logs nothing.
    ///
    /// Once the parser has inserted the body, before those records go out,
    /// the body's window event handler attributes (`<body onload="...">`)
    /// set the window's handlers, replacing any that scripts set before, as
    /// in browsers.
    ///
    /// [`with_document_mut`]: Self::with_document_mut
    pub fn with_parser<R>(
        &self,
        parser: &mut DocumentParser,
        f: impl FnOnce(&mut DocumentParser, &mut Document) -> R,
    ) -> R {
        let dom = Rc::clone(&self.shared().dom);
        let observed = dom.is_observed();
        parser.log_changes(observed);
        let result = self.with_document_mut(|doc| f(parser, doc));
        // Before any script runs, which may detach them.
        dom.hold_for_parser(parser.held_nodes());
        self.note_parsed_body();
        if observed {
            dom.parser_changes(&dom.doc(), &parser.take_changes());
            if dom.has_pending_records() {
                self.with(|ctx| {
                    if let Err(e) = mutation::deliver(&ctx, &dom) {
                        self.shared().report_uncaught(&ctx, e);
                    }
                });
                self.perform_microtask_checkpoint();
            }
        }
        result
    }

    /// The first time the document has a body, tells scripts (see
    /// [`with_parser`](Self::with_parser)).
    fn note_parsed_body(&self) {
        let dom = &self.shared().dom;
        if dom.body_seen.get() {
            return;
        }
        let Some(body) = dom.doc().body() else {
            return;
        };
        dom.body_seen.set(true);
        self.body_inserted(body);
    }

    /// Keeps `node`, and so the tree it is in, from being freed while the
    /// returned [`HeldNode`] lives, so the host can keep its id across
    /// script runs that may detach it (a script element waiting to run,
    /// say). Nodes are otherwise freed once they are detached and script
    /// can no longer reach them. A node may be held any number of times.
    pub fn hold_node(&self, node: NodeId) -> HeldNode {
        HeldNode::new(&self.shared().dom, node)
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
        self.with(|ctx| {
            wrapper::detach_all(&ctx, dom);
            // Name ids are the old document's.
            if let Some(data) = ctx.userdata::<wrapper::DomData<'_>>() {
                data.members.forget_names();
            }
        });
        let old = std::mem::replace(&mut *dom.doc.borrow_mut(), document);
        dom.candidates.borrow_mut().clear();
        dom.held.borrow_mut().clear();
        dom.parser_held.borrow_mut().clear();
        dom.generation.set(dom.generation.get() + 1);
        dom.gc_at
            .set(next_gc(dom.doc.borrow().len(), dom.node_limit));
        dom.selectors.borrow_mut().clear();
        // The document node keeps its wrapper, and so its observers.
        let root = dom.doc.borrow().root();
        dom.observed.borrow_mut().retain(|&n| n == root);
        dom.owners.borrow_mut().clear();
        dom.pending.borrow_mut().clear();
        dom.ce_custom.borrow_mut().clear();
        dom.ce_reactions.borrow_mut().clear();
        dom.body_seen.set(false);
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
            const other = document.implementation.createHTMLDocument('');
            pool.push(other.body, other.createElement('p'), other.createDocumentFragment());
            const host = document.createElement('my-host');
            pool.push(host, host.attachShadow({ mode: 'open' }));
            customElements.define('x-r', class extends HTMLElement {
              connectedCallback() { this.seen = (this.seen || 0) + 1; }
              disconnectedCallback() { this.gone = true; }
            });
            for (let i = 0; i < 3; i++) pool.push(document.createElement('x-r'));
            const pick = () => pool[rand(pool.length)];
            globalThis.step = () => {
              const a = pick(), b = pick();
              const op = rand(14);
              try {
                switch (op) {
                  case 0: a.appendChild(b); break;
                  case 1: a.insertBefore(b, a.firstChild); break;
                  case 2: if (b.parentNode) b.remove(); break;
                  case 3: a.innerHTML = '<i><b></b></i><x-r></x-r><u></u>'; break;
                  case 4: if (b.parentNode) b.parentNode.replaceChild(a, b); break;
                  case 5: { const c = a.firstChild; if (c) { pool.push(c); c.k = 1; } break; }
                  case 6: { const c = a.querySelector('*'); if (c) pool.push(c); break; }
                  case 7: a.textContent = 'x'; break;
                  case 8: a.replaceChildren(b, 'y'); break;
                  case 9: if (pool.length > 10) pool.splice(rand(pool.length - 1) + 1, 1); break;
                  case 10: a.append(b.cloneNode(true)); break;
                  case 11: pool.push(document.adoptNode(b)); break;
                  case 12: { const c = other.importNode(b, true); a.append(c); pool.push(c); break; }
                  case 13: if (!a.shadowRoot && a.attachShadow) { pool.push(a.attachShadow({ mode: 'open', clonable: true })); } break;
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
