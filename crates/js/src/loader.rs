//! What the page loader (`dasher-page`) needs from the runtime: the
//! document's lifecycle, running script elements, taking over the scripts
//! that scripts insert, and `document.write()` during parsing.
//!
//! The loader itself (fetching, parsing, script ordering) lives in the page
//! crate. These are the hooks it drives the JS side with.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dasher_dom::NodeId;
use rquickjs::{Ctx, Function, JsLifetime, Object, Value};

use crate::dom;
use crate::error::ScriptError;
use crate::events::{self, HostTarget};
use crate::modules;
use crate::remote::{PendingOperation, TaskSender};
use crate::runtime::{JsRuntime, Shared, eval_options};

/// `document.readyState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadyState {
    /// The document is being parsed.
    Loading,
    /// Parsing is done; deferred scripts and `DOMContentLoaded` come next.
    Interactive,
    /// The `load` event is about to fire or has fired.
    Complete,
}

impl ReadyState {
    /// The value scripts see, e.g. `"loading"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Interactive => "interactive",
            Self::Complete => "complete",
        }
    }
}

/// Where the host fires an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventTarget {
    /// The global object.
    Window,
    /// `document`.
    Document,
    /// A node of the page's document.
    Node(NodeId),
}

/// What `document.write()` does with its markup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DocumentWrite {
    /// No parser is running: the markup is parsed and inserted after the
    /// running script, or at the end of the body.
    #[default]
    Insert,
    /// A script the parser ran is running: the markup is kept for the
    /// parser, to be read with [`JsRuntime::take_document_write`].
    Capture,
    /// The parser is running but cannot take markup from this script (an
    /// async or deferred one): the call does nothing, as in browsers.
    Ignore,
}

/// Facts about the document that come from its response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentInfo {
    /// `document.characterSet`, e.g. `"UTF-8"` or `"windows-1252"`.
    pub character_set: String,
    /// `document.contentType`, e.g. `"text/html"`.
    pub content_type: String,
    /// `document.referrer`: the URL of the page that navigated here, or
    /// empty.
    pub referrer: String,
}

impl Default for DocumentInfo {
    fn default() -> Self {
        Self {
            character_set: String::from("UTF-8"),
            content_type: String::from("text/html"),
            referrer: String::new(),
        }
    }
}

/// A script element a script inserted into the document, handed to the
/// page loader to prepare.
///
/// Only external scripts and non-classic ones come here: inline classic
/// scripts run as they are inserted.
#[derive(Debug)]
pub struct InsertedScript {
    /// The script element.
    pub element: NodeId,
    /// The element's "non-blocking" flag: set on scripts created by script
    /// until `async` is assigned. Non-blocking external scripts run as soon
    /// as they load; the others run in insertion order.
    pub non_blocking: bool,
    /// Keeps the event loop from going idle, or skipping ahead in virtual
    /// time, until the loader is done with the script. Drop it once the
    /// script has run or failed.
    pub operation: PendingOperation,
}

/// Loader state shared with the native functions.
#[derive(Default)]
pub(crate) struct LoaderState {
    take_over: Cell<bool>,
    inserted: RefCell<Vec<InsertedScript>>,
    inserted_notify: tokio::sync::Notify,
    write_mode: Cell<DocumentWrite>,
    written: RefCell<String>,
    ready_state: Cell<Option<ReadyState>>,
}

/// The JS functions the loader calls (see `loader.js`).
#[derive(Clone)]
pub(crate) struct LoaderHooks<'js> {
    set_ready_state: Function<'js>,
    set_current_script: Function<'js>,
    set_document_info: Function<'js>,
    upgrade_custom_elements: Function<'js>,
    body_inserted: Function<'js>,
    fire: Function<'js>,
}

// SAFETY: `LoaderHooks` only holds values bound to `'js`, and `Changed` is the
// same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for LoaderHooks<'js> {
    type Changed<'to> = LoaderHooks<'to>;
}

impl<'js> LoaderHooks<'js> {
    pub(crate) fn from_glue(internals: &Object<'js>) -> rquickjs::Result<Self> {
        let hooks: Object<'js> = internals.get("loader")?;
        Ok(Self {
            set_ready_state: hooks.get("setReadyState")?,
            set_current_script: hooks.get("setCurrentScript")?,
            set_document_info: hooks.get("setDocumentInfo")?,
            upgrade_custom_elements: hooks.get("upgradeCustomElements")?,
            body_inserted: hooks.get("bodyInserted")?,
            fire: hooks.get("fire")?,
        })
    }

    pub(crate) fn get(ctx: &Ctx<'js>) -> rquickjs::Result<Self> {
        ctx.userdata::<LoaderHooks<'js>>()
            .map(|h| h.clone())
            .ok_or(rquickjs::Error::Unknown)
    }
}

/// Adds the loader's host functions to `native` as `native.loader`.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> rquickjs::Result<()> {
    let loader = Object::new(ctx.clone())?;
    let s = Rc::clone(shared);
    loader.set(
        "scriptInserted",
        Function::new(
            ctx.clone(),
            move |node: dom::Node<'js>, non_blocking: bool| {
                let state = &s.loader;
                let Some(element) = node.borrow().id() else {
                    return;
                };
                if !state.take_over.get() {
                    return;
                }
                let operation = TaskSender::new(Arc::clone(&s.remote)).begin_operation();
                state.inserted.borrow_mut().push(InsertedScript {
                    element,
                    non_blocking,
                    operation,
                });
                state.inserted_notify.notify_one();
            },
        )?,
    )?;
    let s = Rc::clone(shared);
    loader.set(
        "write",
        Function::new(ctx.clone(), move |html: String| {
            let state = &s.loader;
            match state.write_mode.get() {
                DocumentWrite::Insert => false,
                DocumentWrite::Capture => {
                    state.written.borrow_mut().push_str(&html);
                    true
                }
                DocumentWrite::Ignore => true,
            }
        })?,
    )?;
    native.set("loader", loader)
}

impl JsRuntime {
    /// `document.readyState`. Without a page loader the document counts as
    /// complete.
    pub fn ready_state(&self) -> ReadyState {
        self.shared()
            .loader
            .ready_state
            .get()
            .unwrap_or(ReadyState::Complete)
    }

    /// Sets `document.readyState` and, if it changed, fires
    /// `readystatechange` at the document, as a task.
    pub fn set_ready_state(&self, state: ReadyState) {
        self.shared().loader.ready_state.set(Some(state));
        let _ = self.run_task(|ctx| {
            LoaderHooks::get(ctx)?
                .set_ready_state
                .call::<_, ()>((state.as_str(),))
        });
        self.perform_microtask_checkpoint();
    }

    /// Upgrades the custom elements in the document that a definition
    /// covers but that are not custom yet, then runs a microtask
    /// checkpoint, so components render. The page loader calls this after
    /// each parsing step: elements it parses are not upgraded otherwise.
    /// Does nothing until the page defines a custom element.
    pub fn upgrade_custom_elements(&self) {
        if !self.shared().dom.custom_elements_defined() {
            return;
        }
        let _ = self.run_task(|ctx| {
            LoaderHooks::get(ctx)?
                .upgrade_custom_elements
                .call::<_, ()>(())
        });
        self.perform_microtask_checkpoint();
    }

    /// Sets what `document.characterSet`, `contentType` and `referrer`
    /// report.
    pub fn set_document_info(&self, info: &DocumentInfo) {
        let _ = self.run_task(|ctx| {
            let object = Object::new(ctx.clone())?;
            object.set("characterSet", info.character_set.as_str())?;
            object.set("contentType", info.content_type.as_str())?;
            object.set("referrer", info.referrer.as_str())?;
            LoaderHooks::get(ctx)?
                .set_document_info
                .call::<_, ()>((object,))
        });
    }

    /// Tells scripts the parser has inserted `body`, the document's
    /// `<body>` (or `<frameset>`) element, so its window event handler
    /// attributes set the window's handlers.
    pub(crate) fn body_inserted(&self, body: NodeId) {
        let _ = self.run_task(|ctx| {
            let body = dom::wrap(ctx, &self.shared().dom, body)?;
            LoaderHooks::get(ctx)?.body_inserted.call::<_, ()>((body,))
        });
    }

    /// Fires a trusted event named `event` at `target`, as a task followed
    /// by a microtask checkpoint. `pageshow` and `pagehide` are
    /// `PageTransitionEvent`s; everything else is a plain `Event`. Returns
    /// whether no listener canceled it.
    ///
    /// When no listener could hear the event (none for its type on its
    /// path, nor a handler attribute that would set one), nothing runs: no
    /// event object is made and there is no task.
    pub fn fire_event(
        &self,
        target: EventTarget,
        event: &str,
        bubbles: bool,
        cancelable: bool,
    ) -> bool {
        let target = match target {
            EventTarget::Window => HostTarget::Window,
            EventTarget::Document => HostTarget::Node(self.shared().dom.doc().root()),
            EventTarget::Node(id) => HostTarget::Node(id),
        };
        if !self.with(|ctx| events::reaches(&ctx, target, event)) {
            return true;
        }
        let mut not_canceled = true;
        let _ = self.run_task(|ctx| {
            not_canceled = if event == "pageshow" || event == "pagehide" {
                let hooks = LoaderHooks::get(ctx)?;
                let target = match target {
                    HostTarget::Window => Value::new_null(ctx.clone()),
                    HostTarget::Node(id) => dom::wrap(ctx, &self.shared().dom, id)?.into_value(),
                };
                hooks.fire.call((target, event, bubbles, cancelable))?
            } else {
                events::fire(ctx, target, event, bubbles, cancelable)?
            };
            Ok(())
        });
        self.perform_microtask_checkpoint();
        not_canceled
    }

    /// Runs the classic script `source` for the script element `element`,
    /// as a task followed by a microtask checkpoint, with
    /// `document.currentScript` set to the element while it runs.
    ///
    /// `url` is the script's URL (the document's, for an inline script),
    /// used as the file name in stack traces and error events. Errors are
    /// reported as [`run_script`](Self::run_script) reports them.
    pub fn run_script_element(
        &self,
        element: NodeId,
        source: &str,
        url: &str,
    ) -> Result<(), ScriptError> {
        let result = self.run_task(|ctx| {
            let hooks = LoaderHooks::get(ctx)?;
            let script = dom::wrap(ctx, &self.shared().dom, element)?;
            let previous: Value<'_> = hooks.set_current_script.call((script,))?;
            let source = modules::rewrite_classic(source);
            let result = ctx.eval_with_options::<(), _>(&*source, eval_options(url));
            // Take the exception off the context before calling back into JS.
            let thrown = result.is_err().then(|| ctx.catch());
            hooks.set_current_script.call::<_, ()>((previous,))?;
            match thrown {
                Some(exception) => Err(ctx.throw(exception)),
                None => Ok(()),
            }
        });
        self.perform_microtask_checkpoint();
        result
    }

    /// Hands the external and non-classic scripts that scripts insert to
    /// the host, as [`InsertedScript`]s, instead of leaving them alone.
    /// The page loader calls this before it runs anything.
    pub fn take_over_inserted_scripts(&self) {
        self.shared().loader.take_over.set(true);
    }

    /// Removes and returns the scripts inserted since the last call, in
    /// insertion order.
    pub fn take_inserted_scripts(&self) -> Vec<InsertedScript> {
        self.shared().loader.inserted.take()
    }

    /// Waits until a script has been inserted and not yet taken.
    pub async fn script_inserted(&self) {
        let state = &self.shared().loader;
        while state.inserted.borrow().is_empty() {
            state.inserted_notify.notified().await;
        }
    }

    /// Sets what `document.write()` does from now on.
    pub fn set_document_write(&self, mode: DocumentWrite) {
        self.shared().loader.write_mode.set(mode);
    }

    /// Removes and returns the markup captured from `document.write()`
    /// while in [`DocumentWrite::Capture`] mode.
    pub fn take_document_write(&self) -> String {
        self.shared().loader.written.take()
    }
}
