//! Natives for shadow roots, which live in the document arena (see
//! `dasher_dom`'s `attach_shadow`). A shadow tree belongs to its host's
//! wrapper group, like template contents to their template's.

use std::rc::Rc;

use dasher_dom::{ShadowRootInit, ShadowRootMode, ShadowRoots, is_valid_custom_element_name};
use rquickjs::{Array, Ctx, Result};

use super::DomState;
use super::natives::{def, dom_error, element, id, wrap_opt};
use super::wrapper::{Node, wrap};

pub(super) fn install<'js>(obj: &rquickjs::Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "attachShadow",
        move |ctx: Ctx<'js>,
              host: Node<'js>,
              closed: bool,
              delegates_focus: bool,
              clonable: bool,
              serializable: bool|
              -> Result<Node<'js>> {
            let host = element(&ctx, &d, &host)?;
            let init = ShadowRootInit {
                mode: if closed {
                    ShadowRootMode::Closed
                } else {
                    ShadowRootMode::Open
                },
                delegates_focus,
                clonable,
                serializable,
            };
            let root = {
                let mut doc = d.doc_mut();
                let had = doc.shadow_root(host);
                let root = doc.attach_shadow(host, init);
                if had.is_some() {
                    // Taking over a declarative root empties it.
                    d.bump();
                }
                root
            };
            let root = root.map_err(|e| dom_error(&ctx, e))?;
            wrap(&ctx, &d, root)
        },
    )?;
    def(obj, "validCustomElementName", |name: String| {
        is_valid_custom_element_name(&name)
    })?;
    let d = Rc::clone(dom);
    def(obj, "hasShadowRoots", move || d.doc().has_shadow_roots())?;
    let d = Rc::clone(dom);
    def(obj, "shadowRoot", move |ctx: Ctx<'js>, host: Node<'js>| {
        let root = d.doc().shadow_root(id(&ctx, &host)?);
        wrap_opt(&ctx, &d, root)
    })?;
    let d = Rc::clone(dom);
    def(obj, "shadowHost", move |ctx: Ctx<'js>, root: Node<'js>| {
        let host = d.doc().shadow_host(id(&ctx, &root)?);
        wrap_opt(&ctx, &d, host)
    })?;
    let d = Rc::clone(dom);
    // The root's settings as bits: closed, delegatesFocus, clonable and
    // serializable, from the lowest; -1 if `root` is not a shadow root.
    def(
        obj,
        "shadowInit",
        move |ctx: Ctx<'js>, root: Node<'js>| -> Result<i32> {
            let init = d.doc().shadow_root_init(id(&ctx, &root)?);
            Ok(init.map_or(-1, |i| {
                i32::from(i.mode == ShadowRootMode::Closed)
                    | i32::from(i.delegates_focus) << 1
                    | i32::from(i.clonable) << 2
                    | i32::from(i.serializable) << 3
            }))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "getHTML",
        move |ctx: Ctx<'js>,
              n: Node<'js>,
              serializable: bool,
              roots: Array<'js>|
              -> Result<String> {
            let n = id(&ctx, &n)?;
            let roots = roots
                .iter::<Node<'js>>()
                .map(|r| r.and_then(|r| id(&ctx, &r)))
                .collect::<Result<Vec<_>>>()?;
            let shadow = ShadowRoots::Chosen {
                serializable,
                roots: &roots,
            };
            Ok(d.doc().inner_html_with(n, shadow))
        },
    )?;
    Ok(())
}
