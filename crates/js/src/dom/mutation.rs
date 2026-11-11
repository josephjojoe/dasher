//! Mutation records: what a change to the tree looks like to
//! `MutationObserver`.
//!
//! The mutation functions in `natives` report every change the DOM standard
//! queues a mutation record for. Registrations live in JS (`mutation.js`),
//! which tells Rust which nodes have any. When the changed node or one of its
//! ancestors is among them, the change is kept as a [`Record`] of node ids
//! while the document is borrowed, and [`deliver`]ed to the JS hook once the
//! borrow is released. The hook picks the interested observers and queues
//! the records for them. While no node is observed, nothing is collected.
//!
//! The same reports, for nodes in the document, are counted as
//! [`DomActivity`](super::DomActivity) for the page layer.
//!
//! Queued records hold node ids, so no tree is released from the arena
//! while records are queued (see [`DomState::sweep`](super::DomState)).
//! Delivering wraps every node a record names, which keeps it alive from
//! then on.

use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId};
use rquickjs::function::Rest;
use rquickjs::{Ctx, IntoJs, Result, Value};

use super::DomState;
use super::natives::Nullable;
use super::wrapper::{DomData, wrap};

/// The `type` of a mutation record, as passed to the JS hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    ChildList = 0,
    Attributes = 1,
    CharacterData = 2,
}

/// A change that some observer may be interested in.
pub(crate) struct Record {
    /// The observed inclusive ancestors of `target`, nearest first.
    observed: Vec<NodeId>,
    kind: Kind,
    target: NodeId,
    added: Vec<NodeId>,
    removed: Vec<NodeId>,
    prev: Option<NodeId>,
    next: Option<NodeId>,
    /// The attribute's local name and namespace.
    name: Option<String>,
    namespace: Option<String>,
    /// The attribute value or character data before the change.
    old_value: Option<String>,
}

impl Record {
    fn new(observed: Vec<NodeId>, kind: Kind, target: NodeId) -> Self {
        Self {
            observed,
            kind,
            target,
            added: Vec::new(),
            removed: Vec::new(),
            prev: None,
            next: None,
            name: None,
            namespace: None,
            old_value: None,
        }
    }
}

impl DomState {
    /// Counts a change to `target` as activity if it is in the document,
    /// and returns the observed inclusive ancestors of `target`, nearest
    /// first, or `None` if no observer can be interested in it.
    fn interest(&self, doc: &Document, target: NodeId) -> Option<Vec<NodeId>> {
        let observed = self.observed.borrow();
        let mut found = Vec::new();
        let mut top = target;
        for n in std::iter::once(target).chain(doc.ancestors(target)) {
            if !observed.is_empty() && observed.contains(&n) {
                found.push(n);
            }
            top = n;
        }
        if top == doc.root() {
            let mut activity = self.activity.get();
            activity.mutations += 1;
            activity.last_mutation = Some(self.clock.now());
            self.activity.set(activity);
        }
        (!found.is_empty()).then_some(found)
    }

    /// Notes that `added` were inserted into `target` and/or `removed` were
    /// taken out of it, between `prev` and `next`. Call it once the tree has
    /// changed.
    pub(crate) fn child_list(
        &self,
        doc: &Document,
        target: NodeId,
        added: &[NodeId],
        removed: &[NodeId],
        prev: Option<NodeId>,
        next: Option<NodeId>,
    ) {
        if added.is_empty() && removed.is_empty() {
            return;
        }
        let Some(observed) = self.interest(doc, target) else {
            return;
        };
        let mut record = Record::new(observed, Kind::ChildList, target);
        record.added = added.to_vec();
        record.removed = removed.to_vec();
        record.prev = prev;
        record.next = next;
        self.pending.borrow_mut().push(record);
    }

    /// Notes that an attribute of `target` is about to be set or removed.
    /// `old` is its current value, `None` if it is being added.
    pub(crate) fn attribute(
        &self,
        doc: &Document,
        target: NodeId,
        namespace: Name,
        local: &str,
        old: Option<&str>,
    ) {
        let Some(observed) = self.interest(doc, target) else {
            return;
        };
        let mut record = Record::new(observed, Kind::Attributes, target);
        record.name = Some(local.to_owned());
        record.namespace = (namespace != Name::EMPTY).then(|| doc.name_str(namespace).to_owned());
        record.old_value = old.map(str::to_owned);
        self.pending.borrow_mut().push(record);
    }

    /// Notes that the data of a text or comment node is about to change.
    pub(crate) fn character_data(&self, doc: &Document, target: NodeId) {
        let Some(old) = doc.text(target) else {
            return;
        };
        let Some(observed) = self.interest(doc, target) else {
            return;
        };
        let mut record = Record::new(observed, Kind::CharacterData, target);
        record.old_value = Some(old.to_owned());
        self.pending.borrow_mut().push(record);
    }

    /// Whether records are waiting to be delivered.
    pub(crate) fn has_pending_records(&self) -> bool {
        !self.pending.borrow().is_empty()
    }

    /// Marks `id` as having registered observers, or not.
    pub(crate) fn set_observed(&self, id: NodeId, observed: bool) {
        let mut set = self.observed.borrow_mut();
        if observed {
            set.insert(id);
        } else {
            set.remove(&id);
        }
    }
}

/// Hands the queued records to the JS hook. Call it after every change,
/// once the document is no longer borrowed.
pub(crate) fn deliver<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>) -> Result<()> {
    if !dom.has_pending_records() {
        return Ok(());
    }
    let hook = ctx
        .userdata::<DomData<'js>>()
        .and_then(|d| d.mutation_hook.borrow().clone());
    let records = dom.pending.take();
    let Some(hook) = hook else {
        return Ok(());
    };
    // Wrap every record's nodes before running any JS, which keeps them
    // all alive whatever the hook does.
    let mut calls = Vec::with_capacity(records.len());
    for r in records {
        let all = |ids: &[NodeId]| -> Result<Value<'js>> {
            let nodes: Vec<_> = ids
                .iter()
                .map(|&id| wrap(ctx, dom, id))
                .collect::<Result<_>>()?;
            nodes.into_js(ctx)
        };
        let one = |id: Option<NodeId>| -> Result<Value<'js>> {
            Nullable(id.map(|id| wrap(ctx, dom, id)).transpose()?).into_js(ctx)
        };
        calls.push(Rest(vec![
            all(&r.observed)?,
            (r.kind as u32).into_js(ctx)?,
            wrap(ctx, dom, r.target)?.into_value(),
            all(&r.added)?,
            all(&r.removed)?,
            one(r.prev)?,
            one(r.next)?,
            Nullable(r.name).into_js(ctx)?,
            Nullable(r.namespace).into_js(ctx)?,
            Nullable(r.old_value).into_js(ctx)?,
        ]));
    }
    for args in calls {
        hook.call::<_, ()>((args,))?;
    }
    Ok(())
}
