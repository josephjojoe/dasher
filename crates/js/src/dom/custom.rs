//! Custom element reactions: what the registry in `custom_elements.js`
//! needs to hear about from tree mutations.
//!
//! The registry tells Rust the names it has definitions for and which
//! elements are custom. While it has none, nothing here does any work.
//! Otherwise the mutation natives report, while the document is borrowed:
//!
//! - each element that becomes connected and is custom (for
//!   `connectedCallback`) or could be upgraded (its name, or its `is`
//!   attribute, names a definition), in shadow-including tree order;
//! - each custom element that becomes disconnected;
//! - each attribute change on a custom element.
//!
//! These are [`deliver`]ed to the JS hook once the borrow is released, at
//! the end of the DOM call that made them, which is when the HTML standard
//! runs custom element reactions. Queued reactions hold node ids, so no
//! tree is released while any are queued (see `DomState::sweep`).

use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId};
use rquickjs::{Array, Ctx, Function, IntoJs, Object, Result, Value};

use super::DomState;
use super::natives::{Nullable, def, id};
use super::wrapper::{DomData, Node, wrap};

/// A change the custom element registry reacts to.
pub(crate) enum Reaction {
    Connected(NodeId),
    Disconnected(NodeId),
    Attribute {
        element: NodeId,
        local: String,
        namespace: Option<String>,
        old: Option<String>,
        new: Option<String>,
    },
}

impl DomState {
    /// Whether the registry has any definitions.
    pub(crate) fn custom_elements_defined(&self) -> bool {
        !self.ce_names.borrow().is_empty()
    }

    /// Whether `n` is an HTML element that is not custom yet but whose
    /// name, or `is` attribute, names a definition.
    fn could_upgrade(&self, doc: &Document, n: NodeId) -> bool {
        if doc.namespace(n) != Some(Name::NS_HTML) || self.ce_custom.borrow().contains(&n) {
            return false;
        }
        let names = self.ce_names.borrow();
        let local = doc.local_name(n).expect("is an element");
        names.contains(doc.name_str(local))
            || doc.attribute(n, "is").is_some_and(|is| names.contains(is))
    }

    /// The elements at or under `root`, in shadow-including tree order,
    /// that are custom or could be upgraded. With `name`, only those a
    /// definition of that name (for elements named `local`) could upgrade.
    fn candidates(
        &self,
        doc: &Document,
        root: NodeId,
        filter: Option<(&str, &str)>,
    ) -> Vec<NodeId> {
        doc.shadow_including_inclusive_descendants(root)
            .filter(|&n| doc.is_element(n))
            .filter(|&n| match filter {
                None => self.could_upgrade(doc, n),
                Some((name, local)) => {
                    doc.namespace(n) == Some(Name::NS_HTML)
                        && !self.ce_custom.borrow().contains(&n)
                        && doc.local_name(n).is_some_and(|l| doc.name_str(l) == local)
                        && (name == local || doc.attribute(n, "is") == Some(name))
                }
            })
            .collect()
    }

    /// Notes that `roots` (with everything under them) were inserted. Call
    /// it once the tree has changed.
    pub(crate) fn ce_inserted(&self, doc: &Document, roots: &[NodeId]) {
        if !self.custom_elements_defined() {
            return;
        }
        let mut found = Vec::new();
        for &root in roots {
            if !doc.is_connected(root) {
                continue;
            }
            found.extend(
                doc.shadow_including_inclusive_descendants(root)
                    .filter(|&n| {
                        doc.is_element(n)
                            && (self.ce_custom.borrow().contains(&n) || self.could_upgrade(doc, n))
                    })
                    .map(Reaction::Connected),
            );
        }
        self.ce_reactions.borrow_mut().extend(found);
    }

    /// Notes that `roots` were taken out of `parent`. Call it once the tree
    /// has changed, but before the wrapper groups are updated: nothing may
    /// refer to a removed custom element any more, so its wrapper is held
    /// here until its `disconnectedCallback` has run.
    pub(crate) fn ce_removed(
        &self,
        ctx: &Ctx<'_>,
        doc: &Document,
        parent: NodeId,
        roots: &[NodeId],
    ) {
        if self.ce_custom.borrow().is_empty() || !doc.is_connected(parent) {
            return;
        }
        let found: Vec<NodeId> = {
            let custom = self.ce_custom.borrow();
            roots
                .iter()
                .flat_map(|&root| doc.shadow_including_inclusive_descendants(root))
                .filter(|n| custom.contains(n))
                .collect()
        };
        if found.is_empty() {
            return;
        }
        let held = self.cached_all(ctx, &found);
        if let Some(data) = ctx.userdata::<DomData<'_>>() {
            data.ce_held.borrow_mut().extend(held);
        }
        self.ce_reactions
            .borrow_mut()
            .extend(found.into_iter().map(Reaction::Disconnected));
    }

    /// Notes that an attribute of `element` changed from `old` to `new`.
    pub(crate) fn ce_attribute(
        &self,
        doc: &Document,
        element: NodeId,
        namespace: Name,
        local: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        if !self.ce_custom.borrow().contains(&element) {
            return;
        }
        self.ce_reactions.borrow_mut().push(Reaction::Attribute {
            element,
            local: local.to_owned(),
            namespace: (namespace != Name::EMPTY).then(|| doc.name_str(namespace).to_owned()),
            old: old.map(str::to_owned),
            new: new.map(str::to_owned),
        });
    }

    pub(crate) fn has_pending_reactions(&self) -> bool {
        !self.ce_reactions.borrow().is_empty()
    }

    /// Whether `id` is a custom element.
    pub(crate) fn is_custom(&self, id: NodeId) -> bool {
        let custom = self.ce_custom.borrow();
        !custom.is_empty() && custom.contains(&id)
    }

    /// Whether a definition exists for the name `local`, so creating an
    /// element of that name involves the registry.
    pub(crate) fn has_definition(&self, local: &str) -> bool {
        let names = self.ce_names.borrow();
        !names.is_empty() && names.contains(local)
    }

    /// Marks `id` as a custom element, or not.
    pub(crate) fn set_custom(&self, id: NodeId, custom: bool) {
        let mut set = self.ce_custom.borrow_mut();
        if custom {
            set.insert(id);
        } else {
            set.remove(&id);
        }
    }
}

/// Hands the queued reactions to the JS hook. Call it after every change,
/// once the document is no longer borrowed.
pub(crate) fn deliver<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>) -> Result<()> {
    if !dom.has_pending_reactions() {
        return Ok(());
    }
    let reactions = dom.ce_reactions.take();
    let hook = ctx
        .userdata::<DomData<'js>>()
        .and_then(|d| d.ce_hook.borrow().clone());
    let Some(hook) = hook else {
        if let Some(d) = ctx.userdata::<DomData<'js>>() {
            d.ce_held.take();
        }
        return Ok(());
    };
    // Wrap every node before running any JS, which keeps them all alive
    // whatever the hook does.
    let mut calls = Vec::with_capacity(reactions.len());
    for r in reactions {
        let args: (
            u32,
            Node<'js>,
            Value<'js>,
            Value<'js>,
            Value<'js>,
            Value<'js>,
        ) = match r {
            Reaction::Connected(n) | Reaction::Disconnected(n) => {
                let kind = u32::from(matches!(r, Reaction::Disconnected(_)));
                let undefined = Value::new_undefined(ctx.clone());
                (
                    kind,
                    wrap(ctx, dom, n)?,
                    undefined.clone(),
                    undefined.clone(),
                    undefined.clone(),
                    undefined,
                )
            }
            Reaction::Attribute {
                element,
                local,
                namespace,
                old,
                new,
            } => (
                2,
                wrap(ctx, dom, element)?,
                local.into_js(ctx)?,
                Nullable(old).into_js(ctx)?,
                Nullable(new).into_js(ctx)?,
                Nullable(namespace).into_js(ctx)?,
            ),
        };
        calls.push(args);
    }
    let held = ctx
        .userdata::<DomData<'js>>()
        .map(|d| d.ce_held.take())
        .unwrap_or_default();
    for args in calls {
        hook.call::<_, ()>(args)?;
    }
    drop(held);
    Ok(())
}

pub(super) fn install<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    def(
        obj,
        "setCustomElementHook",
        |ctx: Ctx<'js>, hook: Function<'js>| -> Result<()> {
            let data = ctx
                .userdata::<DomData<'js>>()
                .ok_or(rquickjs::Error::Unknown)?;
            data.ce_hook.replace(Some(hook));
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(obj, "defineCustomElement", move |name: String| {
        d.ce_names.borrow_mut().insert(name);
    })?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setCustom",
        move |ctx: Ctx<'js>, n: Node<'js>, custom: bool| -> Result<()> {
            d.set_custom(id(&ctx, &n)?, custom);
            Ok(())
        },
    )?;
    // The elements at or under `root` that a definition could upgrade, in
    // shadow-including tree order: any definition, or the one named `name`
    // for elements named `local`.
    let d = Rc::clone(dom);
    def(
        obj,
        "upgradeCandidates",
        move |ctx: Ctx<'js>,
              root: Node<'js>,
              name: Option<String>,
              local: Option<String>|
              -> Result<Array<'js>> {
            let root = id(&ctx, &root)?;
            let found = {
                let doc = d.doc();
                let filter = name.as_deref().zip(local.as_deref());
                d.candidates(&doc, root, filter)
            };
            let array = Array::new(ctx.clone())?;
            for (i, n) in found.into_iter().enumerate() {
                array.set(i, wrap(&ctx, &d, n)?)?;
            }
            Ok(array)
        },
    )?;
    Ok(())
}
