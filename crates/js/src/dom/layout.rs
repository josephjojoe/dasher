//! The synthetic layout's raw material, and the hook that tells layout
//! observers the document changed.
//!
//! Nothing is laid out. Where geometry matters (the `viewport` visibility,
//! `scrollIntoView`, `IntersectionObserver`), `dom.js` stacks the elements
//! under `<body>` in document order, one line each, with every element
//! spanning the lines of its descendants. [`lines`] gives an element's place
//! in that stack. The default `loaded` visibility stacks only what was
//! inserted since the page was last revealed ([`loaded_lines`], from the
//! document's insertion epochs; see [`JsRuntime::reveal`]).
//!
//! Element boxes are computed here too ([`DomState::element_box`]), for
//! the native layout getters (`clientWidth`, `offsetTop`...) and, through
//! `N.elementBox`, for `getBoundingClientRect` and the rest of `dom.js`:
//!
//! - an element that is not rendered (disconnected, or hidden by its
//!   markup: `:visible` fails) has an empty box;
//! - `<html>` and `<body>` are as wide as the viewport and as tall as the
//!   document ([`DomState::root_height`]);
//! - under `viewport`, any other element under `<body>` sits on its line of
//!   the stack, spanning its descendants' lines;
//! - under `loaded`, it sits on its line of the stack of what was inserted
//!   since the last reveal, at least one line tall;
//! - otherwise (`all`, or outside `<body>`) it is one line at the top.
//!
//! A line is [`LINE`] pixels; boxes are as wide as the viewport.
//!
//! `IntersectionObserver` must look again whenever the tree changes. Rather
//! than have every mutation call into JS, the event loop calls the layout
//! hook (set with `native.dom.setLayoutHook`) once the task queue has
//! drained, if the document changed since the hook last ran: much like a
//! browser updating intersections in its rendering step.

use std::collections::HashMap;
use std::rc::Rc;

use dasher_css::SelectorList;
use dasher_dom::{Document, Name, NodeId};
use rquickjs::{Ctx, Object, Result, qjs};

use super::DomState;
use super::members::{Call, Outcome, Ret};
use super::wrapper::{DomData, Node};
use crate::Visibility;
use crate::runtime::JsRuntime;

/// The height of a line of the stacked layouts, in CSS pixels.
pub(crate) const LINE: i32 = 20;

/// The viewport and the visibility policy, which decide where boxes go.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) visibility: Visibility,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            width: 1920,
            height: 1080,
            visibility: Visibility::default(),
        }
    }
}

/// What [`DomState::element_box`] keeps between calls.
#[derive(Default)]
pub(crate) struct LayoutCache {
    /// `:visible`, parsed once.
    visible: std::cell::OnceCell<SelectorList>,
    /// `body *`, for the document height under `all`.
    body_descendants: std::cell::OnceCell<SelectorList>,
    /// The document height, and the document version and insertion epoch
    /// it was computed for.
    root: std::cell::Cell<Option<(u32, u16, i32)>>,
}

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

/// Each target's place in the `loaded` visibility's layout, as `[line,
/// lines]` pairs in one flat list. Only elements inserted in the
/// document's current [insertion epoch](Document::insertion_epoch) (since
/// the page last revealed what it holds), and elements inside them, take up
/// a line; everything else is stacked at the top, taking none. So an
/// element's line is the number of such appended elements before it in
/// document order, and its span the number in its own subtree, itself
/// included. Shadow trees are walked before their host's children.
/// Elements outside `<body>` get `[-1, 0]`. While nothing has been
/// inserted in the current epoch, every target gets `[0, 0]` at once.
pub(crate) fn loaded_lines(doc: &Document, targets: &[NodeId]) -> Vec<i32> {
    let epoch = doc.insertion_epoch();
    if epoch == 0 || !doc.inserted_in_epoch() {
        return vec![0; 2 * targets.len()];
    }
    let mut out = [-1, 0].repeat(targets.len());
    let Some(body) = doc.body() else {
        return out;
    };
    let mut wanted: HashMap<NodeId, Vec<usize>> = HashMap::with_capacity(targets.len());
    for (i, &t) in targets.iter().enumerate() {
        wanted.entry(t).or_default().push(i);
    }
    let mut left = wanted.len();
    let appended = |node: NodeId| doc.node_epoch(node) == epoch;
    // An open element: its next child to visit, whether that cursor is
    // still in its shadow tree, its line, and whether it is appended.
    struct Open {
        node: NodeId,
        next: Option<NodeId>,
        in_shadow: bool,
        line: i32,
        appended: bool,
    }
    let open_element = |node: NodeId, line: i32, appended: bool| {
        let shadow = doc.shadow_root(node);
        Open {
            node,
            next: match shadow {
                Some(root) => doc.first_child(root),
                None => doc.first_child(node),
            },
            in_shadow: shadow.is_some(),
            line,
            appended,
        }
    };
    let body_appended = appended(body);
    let mut next = i32::from(body_appended);
    let mut open = vec![open_element(body, 0, body_appended)];
    while let Some(top) = open.last_mut() {
        match top.next {
            Some(child) => {
                top.next = doc.next_sibling(child);
                if doc.is_element(child) {
                    let is_appended = top.appended || appended(child);
                    let element = open_element(child, next, is_appended);
                    next += i32::from(is_appended);
                    open.push(element);
                }
            }
            None if top.in_shadow => {
                top.in_shadow = false;
                top.next = doc.first_child(top.node);
            }
            None => {
                let done = open.pop().expect("not empty");
                if let Some(indices) = wanted.get(&done.node) {
                    for &i in indices {
                        out[2 * i] = done.line;
                        out[2 * i + 1] = next - done.line;
                    }
                    left -= 1;
                    if left == 0 {
                        break;
                    }
                }
            }
        }
    }
    out
}

impl DomState {
    /// Whether `el` is visible as far as its markup goes (`:visible`).
    pub(crate) fn visible(&self, doc: &Document, el: NodeId) -> bool {
        let visible = self
            .layout
            .visible
            .get_or_init(|| SelectorList::parse(":visible").expect("`:visible` parses"));
        visible.matches(doc, el)
    }

    /// Whether `el` is rendered: connected and visible.
    pub(crate) fn rendered(&self, doc: &Document, el: NodeId) -> bool {
        doc.is_connected(el) && self.visible(doc, el)
    }

    /// The height of the root element (and of `<body>`): the content's,
    /// and at least the viewport's.
    pub(crate) fn root_height(&self, doc: &Document) -> i32 {
        let g = self.geometry;
        let version = self.version();
        let epoch = if g.visibility == Visibility::Loaded {
            doc.insertion_epoch()
        } else {
            0
        };
        if let Some((v, e, h)) = self.layout.root.get() {
            if v == version && e == epoch {
                return h;
            }
        }
        let body = doc.body();
        let height = match g.visibility {
            Visibility::Viewport => body.map_or(0, |b| lines(doc, &[b])[1]) * LINE,
            Visibility::Loaded => {
                2 * g.height + body.map_or(0, |b| loaded_lines(doc, &[b])[1].max(0)) * LINE
            }
            Visibility::All => {
                let list = self
                    .layout
                    .body_descendants
                    .get_or_init(|| SelectorList::parse("body *").expect("`body *` parses"));
                let count = list.query_all(doc, doc.root()).count();
                i32::try_from(count).unwrap_or(i32::MAX / LINE) * LINE
            }
        };
        let height = height.max(g.height);
        self.layout.root.set(Some((version, epoch, height)));
        height
    }

    /// `el`'s box, `[x, y, width, height]` in document coordinates (see the
    /// module docs).
    pub(crate) fn element_box(&self, doc: &Document, el: NodeId) -> [i32; 4] {
        if !self.rendered(doc, el) {
            return [0; 4];
        }
        let g = self.geometry;
        if doc.is_html(el, Name::HTML) || doc.is_html(el, Name::BODY) {
            return [0, 0, g.width, self.root_height(doc)];
        }
        match g.visibility {
            Visibility::Viewport => {
                let l = lines(doc, &[el]);
                if l[0] >= 0 {
                    return [0, l[0] * LINE, g.width, l[1] * LINE];
                }
            }
            Visibility::Loaded => {
                let l = loaded_lines(doc, &[el]);
                return [0, l[0].max(0) * LINE, g.width, l[1].max(1) * LINE];
            }
            Visibility::All => {}
        }
        [0, 0, g.width, LINE]
    }

    /// Whether `el` scrolls the viewport: `<html>`, or `<body>` as the
    /// root's child.
    fn is_root_scroller(doc: &Document, el: NodeId) -> bool {
        doc.is_html(el, Name::HTML)
            || doc.is_html(el, Name::BODY)
                && doc
                    .parent(el)
                    .is_some_and(|p| Some(p) == doc.document_element())
    }

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
    /// Counts everything in the document now as seen, for the
    /// [`Visibility::Loaded`](crate::Visibility::Loaded) policy: it is all
    /// in view from now on, and what is inserted after this call is below
    /// the fold until the next one. The page loader calls it once the load
    /// event has run, and auto-scroll before each round. Under the other
    /// policies it changes nothing that scripts can see.
    ///
    /// `IntersectionObserver`s look again once the task queue next drains.
    pub fn reveal(&self) {
        let dom = &self.shared().dom;
        dom.doc_mut().begin_insertion_epoch();
        // Geometry changed without the tree changing: make the layout hook
        // run on the next drain.
        dom.layout_seen.set(dom.version().wrapping_sub(1));
    }

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

// -------------------------------------------------------------------------
// The native layout getters (see the `members` module).

pub(super) fn client_width<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    if doc_is_root_html(c, n) {
        return Ok(Ret::Int(c.dom.geometry.width));
    }
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[2]))
}

pub(super) fn client_height<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    if doc_is_root_html(c, n) {
        return Ok(Ret::Int(c.dom.geometry.height));
    }
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[3]))
}

/// Whether `n` is `<html>`, whose client box is the viewport.
fn doc_is_root_html(c: &Call<'_>, n: NodeId) -> bool {
    DomState::is_root_scroller(&c.doc, n) && c.doc.is_html(n, Name::HTML)
}

pub(super) fn box_left<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[0]))
}

pub(super) fn box_top<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[1]))
}

pub(super) fn box_width<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[2]))
}

pub(super) fn box_height<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Int(c.dom.element_box(&c.doc, n)[3]))
}

/// `N.elementBox(el)` and `N.rootHeight()`, for `dom.js`.
pub(super) fn install<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    let f = rquickjs::Function::new(
        obj.ctx().clone(),
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Vec<i32>> {
            let id = super::natives::id(&ctx, &n)?;
            Ok(d.element_box(&d.doc(), id).to_vec())
        },
    )?;
    obj.set("elementBox", f)?;
    let d = Rc::clone(dom);
    let f = rquickjs::Function::new(obj.ctx().clone(), move || d.root_height(&d.doc()))?;
    obj.set("rootHeight", f)
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
    fn only_appended_elements_take_lines() {
        let mut doc = Document::parse(
            "<body><div id=a><p id=b>one</p></div><div id=c></div><footer id=e></footer></body>",
        );
        let [a, b, c, e] = ["a", "b", "c", "e"].map(|id| find(&doc, id));
        // Before any epoch, and while nothing is inserted in it, all at 0.
        assert_eq!(loaded_lines(&doc, &[a, e]), [0, 0, 0, 0]);
        doc.begin_insertion_epoch();
        assert_eq!(loaded_lines(&doc, &[a, e]), [0, 0, 0, 0]);
        // Three appended elements in c push the footer three lines down;
        // what came before stays put.
        for _ in 0..2 {
            let item = doc.create_element("li");
            let inner = doc.create_element("b");
            doc.append_child(item, inner).unwrap();
            doc.append_child(c, item).unwrap();
        }
        let first = doc.first_child(c).unwrap();
        assert_eq!(
            loaded_lines(&doc, &[a, b, c, first, e]),
            [0, 0, 0, 0, 0, 4, 0, 2, 4, 0]
        );
        // A new epoch counts all of it as seen.
        doc.begin_insertion_epoch();
        assert_eq!(loaded_lines(&doc, &[e]), [0, 0]);
    }

    #[test]
    fn shadow_trees_take_lines_before_their_hosts_children() {
        let mut doc =
            Document::parse("<body><x-feed id=h><p id=light></p></x-feed><p id=after></p></body>");
        let [host, light, after] = ["h", "light", "after"].map(|id| find(&doc, id));
        let root = doc
            .attach_shadow(host, dasher_dom::ShadowRootInit::open())
            .unwrap();
        doc.begin_insertion_epoch();
        for _ in 0..3 {
            let item = doc.create_element("li");
            doc.append_child(root, item).unwrap();
        }
        assert_eq!(
            loaded_lines(&doc, &[host, light, after]),
            [0, 3, 3, 0, 3, 0]
        );
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
