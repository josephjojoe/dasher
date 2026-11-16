//! ES modules: the module map, fetching module graphs through the page's
//! session, and the hooks QuickJS loads modules through.
//!
//! QuickJS asks for a module's source synchronously, while linking, and
//! cannot wait for the network. So a graph is fetched before QuickJS sees
//! it: each module's source is scanned for its static imports
//! ([`lexer`]), those are resolved ([`resolve`]) and fetched, in parallel,
//! until every module the graph reaches has arrived. Only then is the root
//! loaded, and QuickJS's loader callback finds every source waiting in the
//! module map. Once compiled, a module lives in QuickJS's own list, and the
//! map drops its source.
//!
//! Dynamic `import()` cannot wait either: QuickJS resolves it in a job
//! that loads the module there and then. So before a script is compiled,
//! its `import(` calls are renamed to [`DYNAMIC_IMPORT`], a global that
//! fetches the requested graph asynchronously and then loads it. The name
//! has as many characters as `import`, so error positions do not move.
//! `import()` the scanner misses (in `eval` code, say) still works through
//! QuickJS: the loader fetches what is missing then, blocking the page
//! while it does.

mod lexer;
mod resolve;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::Arc;

use dasher_dom::{Name, NodeId};
use dasher_net::{Credentials, Request, RequestMode, ResourceType, Session};
use rquickjs::function::{Opt, This};
use rquickjs::{
    Coerced, Ctx, Exception, FromJs, Function, JsLifetime, Object, Promise, Value, qjs,
};
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use url::Url;

use crate::console::{ConsoleLevel, ConsoleMessage};
use crate::data_url;
use crate::dom;
use crate::events::{self, HostTarget};
use crate::net::decode_utf8;
use crate::remote::TaskSender;
use crate::runtime::{JsRuntime, Shared, note_module_error};

pub(crate) use lexer::may_import;
use resolve::{ImportMap, RawEntries, RawImportMap};

/// The global that dynamic `import(` calls are renamed to call. It must be
/// exactly as long as `import`.
pub(crate) const DYNAMIC_IMPORT: &str = "__dimp";

/// A module graph the host asked for with
/// [`JsRuntime::fetch_module_graph`] or
/// [`JsRuntime::fetch_inline_module_graph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModuleGraph(u32);

/// Whether a module graph has finished loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleGraphStatus {
    /// Modules are still being fetched.
    Loading,
    /// Every module arrived: the graph can run.
    Ready,
    /// A module could not be fetched. The reason was logged to the
    /// console.
    Failed,
}

/// Whether module requests send credentials (cookies) to other origins,
/// from a module script's `crossorigin` attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModuleCredentials {
    /// No `crossorigin`, or `crossorigin="anonymous"`: cookies go only to
    /// the document's own origin.
    #[default]
    SameOrigin,
    /// `crossorigin="use-credentials"`: cookies go everywhere.
    Include,
}

/// A module in the module map, keyed by the URL it was requested from (or,
/// for an inline module script, a name of its own).
enum Entry {
    Fetching,
    Fetched(Fetched),
    /// The fetch failed, for this reason.
    Failed(String),
    /// Compiled: QuickJS has it now.
    Declared,
}

struct Fetched {
    /// The source, with dynamic imports renamed.
    source: String,
    /// The URLs its static imports resolve to. Specifiers that do not
    /// resolve are left out: linking reports them.
    deps: Vec<String>,
}

/// What a module graph is for, which decides what happens when it settles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// A module script the host runs when it is ready.
    Host,
    /// A dynamic `import()`, resolved once the graph has loaded.
    Import,
    /// `<link rel=modulepreload>`: fetches only the module itself, then
    /// fires `load` or `error` at the link.
    Preload(NodeId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GraphState {
    Loading,
    Ready,
    /// Which module failed, and why.
    Failed(String, String),
}

struct Graph {
    root: String,
    credentials: ModuleCredentials,
    purpose: Purpose,
    state: GraphState,
}

/// The module side of a runtime. Holds no JS values.
#[derive(Default)]
pub(crate) struct ModuleState {
    map: RefCell<HashMap<String, Entry>>,
    /// Base URLs of modules whose name is not their base URL: inline
    /// module scripts, and modules whose request was redirected.
    bases: RefCell<HashMap<String, Url>>,
    import_map: RefCell<ImportMap>,
    graphs: RefCell<HashMap<u32, Graph>>,
    last_graph: Cell<u32>,
    inline_count: Cell<u32>,
    /// Wakes the host when a graph it asked for settles.
    settled: Notify,
    /// Fetches in progress, by URL, to cancel when the page goes away.
    in_flight: RefCell<HashMap<String, AbortHandle>>,
    /// Modulepreload links already handled, which the parser and scripts
    /// may both report.
    preload_links: RefCell<HashSet<NodeId>>,
    /// Holds on the links whose module is being fetched, so they are still
    /// there to get their `load` or `error` event if script removes them.
    preload_holds: RefCell<HashMap<NodeId, dom::HeldNode>>,
}

impl ModuleState {
    /// Cancels every fetch in progress, for when the page goes away.
    pub(crate) fn abort_all(&self) {
        for (_, handle) in self.in_flight.borrow_mut().drain() {
            handle.abort();
        }
    }
}

/// JS values of the module system, stored as context userdata.
struct ModuleData<'js> {
    shared: Rc<Shared>,
    /// The resolve and reject functions of dynamic imports waiting for
    /// their graphs, by graph id.
    imports: RefCell<HashMap<u32, (Function<'js>, Function<'js>)>>,
}

// SAFETY: `ModuleData` only holds values bound to `'js` (and an `Rc` with
// no lifetime), and `Changed` is the same struct with the lifetime
// substituted.
unsafe impl<'js> JsLifetime<'js> for ModuleData<'js> {
    type Changed<'to> = ModuleData<'to>;
}

fn shared_of(ctx: &Ctx<'_>) -> Option<Rc<Shared>> {
    ctx.userdata::<ModuleData<'_>>()
        .map(|d| Rc::clone(&d.shared))
}

fn throw_type(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, message)
}

/// A `TypeError` with `message`, as a value.
fn type_error<'js>(ctx: &Ctx<'js>, message: &str) -> Value<'js> {
    let _ = Exception::throw_type(ctx, message);
    ctx.catch()
}

fn log(shared: &Shared, level: ConsoleLevel, text: String) {
    shared.console.borrow_mut().push(ConsoleMessage {
        level,
        text,
        time: shared.event_loop.clock.now(),
    });
}

// ---------------------------------------------------------------------------
// Resolution.

/// The document's base URL: its first `<base href>`, resolved against its
/// URL, or its URL.
fn document_base_url(shared: &Shared) -> Url {
    let url = shared.window.url();
    let doc = shared.dom.doc();
    doc.descendants(doc.root())
        .find(|&n| doc.is_html(n, Name::BASE) && doc.attribute_named(n, Name::HREF).is_some())
        .and_then(|n| doc.attribute_named(n, Name::HREF))
        .and_then(|href| url.join(href).ok())
        .unwrap_or(url)
}

/// The base URL of the script or module called `name`: its response URL
/// for a module, the document's base URL for inline scripts.
fn base_url_of(shared: &Shared, name: &str) -> Url {
    if let Some(base) = shared.modules.bases.borrow().get(name) {
        return base.clone();
    }
    match Url::parse(name) {
        // Inline scripts are named after the document; host scripts have
        // made-up names (`dasher:eval`, which parses as a URL nothing can
        // be resolved against).
        Ok(url) if url != shared.window.url() && url.scheme() != "dasher" => url,
        _ => document_base_url(shared),
    }
}

/// Resolves `specifier` as imported by the script or module `referrer`.
fn resolve(shared: &Shared, referrer: &str, specifier: &str) -> Result<Url, String> {
    let base = base_url_of(shared, referrer);
    shared.modules.import_map.borrow().resolve(specifier, &base)
}

// ---------------------------------------------------------------------------
// Fetching.

/// A module source as fetched, scanned and rewritten.
struct Loaded {
    /// The URL after redirects.
    response_url: Url,
    source: String,
    imports: Vec<String>,
}

impl Loaded {
    fn new(response_url: Url, bytes: &[u8]) -> Self {
        // Module scripts are always UTF-8.
        let (source, imports) = prepare(decode_utf8(bytes));
        Self {
            response_url,
            source,
            imports,
        }
    }
}

/// Scans `source` and renames its dynamic imports. Returns the new source
/// and the specifiers of its static imports.
fn prepare(mut source: String) -> (String, Vec<String>) {
    if !may_import(&source) {
        return (source, Vec::new());
    }
    let scan = lexer::scan(&source);
    for offset in scan.dynamic {
        source.replace_range(offset..offset + DYNAMIC_IMPORT.len(), DYNAMIC_IMPORT);
    }
    (source, scan.imports)
}

/// Renames the dynamic imports in a classic script, so they load
/// asynchronously.
pub(crate) fn rewrite_classic(source: &str) -> std::borrow::Cow<'_, str> {
    if !may_import(source) {
        return source.into();
    }
    let dynamic = lexer::scan(source).dynamic;
    if dynamic.is_empty() {
        return source.into();
    }
    let mut out = source.to_owned();
    for offset in dynamic {
        out.replace_range(offset..offset + DYNAMIC_IMPORT.len(), DYNAMIC_IMPORT);
    }
    out.into()
}

fn request(shared: &Shared, url: Url, credentials: ModuleCredentials) -> Request {
    let document = shared.window.url();
    let credentials = match credentials {
        ModuleCredentials::SameOrigin => Credentials::SameOrigin(document.origin()),
        ModuleCredentials::Include => Credentials::Include,
    };
    // Module scripts are always fetched in CORS mode.
    Request::get(url, ResourceType::Script)
        .with_referrer(document)
        .with_mode(RequestMode::Cors)
        .with_credentials(credentials)
}

async fn fetch(session: Session, request: Request) -> Result<Loaded, String> {
    let response = session.send(request).await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP status {}", status.as_u16()));
    }
    let url = response.url().clone();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    Ok(Loaded::new(url, &bytes))
}

/// A `data:` module, decoded on the spot.
fn load_data_url(url: &Url) -> Result<Loaded, String> {
    let data = data_url::parse(url.as_str()).ok_or("invalid data: URL")?;
    Ok(Loaded::new(url.clone(), &data.body))
}

/// Starts fetching the module at `url` into the module map. The result
/// arrives in a later task.
fn start_fetch(shared: &Rc<Shared>, url: &str, credentials: ModuleCredentials) {
    let modules = &shared.modules;
    modules
        .map
        .borrow_mut()
        .insert(url.to_owned(), Entry::Fetching);
    let key = url.to_owned();
    let deliver_later = |result: Result<Loaded, String>| {
        let key = key.clone();
        shared
            .event_loop
            .queue(Box::new(move |ctx| fetched(&ctx, key, result)));
    };
    let Ok(parsed) = Url::parse(url) else {
        return deliver_later(Err(String::from("invalid URL")));
    };
    match parsed.scheme() {
        "http" | "https" => {}
        "data" => return deliver_later(load_data_url(&parsed)),
        scheme => return deliver_later(Err(format!("unsupported URL scheme '{scheme}:'"))),
    }
    let Some(session) = shared.net.session().cloned() else {
        return deliver_later(Err(String::from("this page has no network access")));
    };
    let Some(runtime) = shared.net.spawn_handle() else {
        return deliver_later(Err(String::from("no Tokio runtime to fetch on")));
    };
    let request = request(shared, parsed, credentials);
    let operation = TaskSender::new(Arc::clone(&shared.remote)).begin_operation();
    let task = runtime.spawn(async move {
        let result = fetch(session, request).await;
        operation.complete(move |ctx| fetched(&ctx, key, result));
    });
    modules
        .in_flight
        .borrow_mut()
        .insert(url.to_owned(), task.abort_handle());
}

/// Fetches a module there and then, blocking the page: for modules QuickJS
/// asks for that were not fetched ahead. The request runs on a thread of
/// its own, as synchronous XHR's does.
fn fetch_blocking(shared: &Shared, url: &str) -> Result<Loaded, String> {
    let parsed = Url::parse(url).map_err(|_| String::from("invalid URL"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        "data" => return load_data_url(&parsed),
        scheme => return Err(format!("unsupported URL scheme '{scheme}:'")),
    }
    // Not a pooled connection: the runtime that drives those is the one
    // this call blocks.
    let session = shared
        .net
        .session()
        .map(Session::unpooled)
        .ok_or("this page has no network access")?;
    let request = request(shared, parsed, ModuleCredentials::SameOrigin);
    std::thread::Builder::new()
        .name("dasher-module-fetch".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(fetch(session, request))
        })
        .map_err(|e| e.to_string())?
        .join()
        .unwrap_or_else(|_| Err(String::from("the fetch thread panicked")))
}

/// Records a fetched module, given its result, and moves the graphs on.
fn fetched(ctx: &Ctx<'_>, url: String, result: Result<Loaded, String>) -> rquickjs::Result<()> {
    let Some(shared) = shared_of(ctx) else {
        return Ok(());
    };
    let modules = &shared.modules;
    modules.in_flight.borrow_mut().remove(&url);
    if matches!(modules.map.borrow().get(&url), Some(Entry::Declared)) {
        // QuickJS could not wait and fetched it itself.
        return Ok(());
    }
    let entry = record(&shared, &url, result);
    modules.map.borrow_mut().insert(url, entry);
    advance(&shared);
    Ok(())
}

/// Turns a fetch result into a module map entry, logging failures.
fn record(shared: &Shared, url: &str, result: Result<Loaded, String>) -> Entry {
    match result {
        Ok(loaded) => {
            let base = loaded.response_url;
            let deps = loaded
                .imports
                .iter()
                .filter_map(|spec| {
                    let map = shared.modules.import_map.borrow();
                    map.resolve(spec, &base).ok().map(String::from)
                })
                .collect();
            if base.as_str() != url {
                shared
                    .modules
                    .bases
                    .borrow_mut()
                    .insert(url.to_owned(), base);
            }
            Entry::Fetched(Fetched {
                source: loaded.source,
                deps,
            })
        }
        Err(reason) => {
            log(
                shared,
                ConsoleLevel::Error,
                format!("Failed to load module script {url}: {reason}"),
            );
            Entry::Failed(reason)
        }
    }
}

// ---------------------------------------------------------------------------
// Graphs.

/// Where a graph stands, and which modules it still needs fetched.
fn walk(map: &HashMap<String, Entry>, graph: &Graph, to_fetch: &mut Vec<String>) -> GraphState {
    let mut seen = HashSet::new();
    let mut stack = vec![graph.root.as_str()];
    let mut loading = false;
    while let Some(url) = stack.pop() {
        if !seen.insert(url) {
            continue;
        }
        match map.get(url) {
            None => {
                to_fetch.push(url.to_owned());
                loading = true;
            }
            Some(Entry::Fetching) => loading = true,
            Some(Entry::Failed(reason)) => {
                return GraphState::Failed(url.to_owned(), reason.clone());
            }
            Some(Entry::Fetched(fetched)) => {
                if !matches!(graph.purpose, Purpose::Preload(_)) {
                    stack.extend(fetched.deps.iter().map(String::as_str));
                }
            }
            Some(Entry::Declared) => {}
        }
    }
    if loading {
        GraphState::Loading
    } else {
        GraphState::Ready
    }
}

/// Moves every loading graph on: starts the fetches they need and settles
/// the ones that are complete.
fn advance(shared: &Rc<Shared>) {
    let modules = &shared.modules;
    let mut fetches: Vec<(String, ModuleCredentials)> = Vec::new();
    let mut settled = Vec::new();
    {
        let map = modules.map.borrow();
        let mut graphs = modules.graphs.borrow_mut();
        for (&id, graph) in graphs.iter_mut() {
            if graph.state != GraphState::Loading {
                continue;
            }
            let mut to_fetch = Vec::new();
            graph.state = walk(&map, graph, &mut to_fetch);
            fetches.extend(to_fetch.into_iter().map(|url| (url, graph.credentials)));
            if graph.state != GraphState::Loading {
                settled.push((id, graph.purpose));
            }
        }
    }
    let mut started = HashSet::new();
    for (url, credentials) in fetches {
        if started.insert(url.clone()) {
            start_fetch(shared, &url, credentials);
        }
    }
    let any_settled = !settled.is_empty();
    for (id, purpose) in settled {
        match purpose {
            Purpose::Host => {}
            Purpose::Import => shared
                .event_loop
                .queue(Box::new(move |ctx| finish_import(&ctx, id))),
            Purpose::Preload(link) => shared
                .event_loop
                .queue(Box::new(move |ctx| finish_preload(&ctx, id, link))),
        }
    }
    if any_settled {
        modules.settled.notify_one();
    }
}

fn start_graph(
    shared: &Rc<Shared>,
    root: String,
    credentials: ModuleCredentials,
    purpose: Purpose,
) -> u32 {
    let modules = &shared.modules;
    let id = modules.last_graph.get().wrapping_add(1);
    modules.last_graph.set(id);
    modules.graphs.borrow_mut().insert(
        id,
        Graph {
            root,
            credentials,
            purpose,
            state: GraphState::Loading,
        },
    );
    advance(shared);
    id
}

/// Loads, links and evaluates the module `name`, whose graph has been
/// fetched. Returns the promise of its namespace, which settles once it
/// has run (top-level `await` included).
fn load_root<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Promise<'js>> {
    let name = CString::new(name)?;
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `raw` is the live context `ctx` wraps, locked for this call.
    // The empty base name tells `normalize` that `name` is resolved
    // already. The returned value is owned and handed to `Value::from_raw`.
    let value = unsafe {
        let value = qjs::JS_LoadModule(raw, c"".as_ptr(), name.as_ptr());
        if qjs::JS_IsException(value) {
            return Err(rquickjs::Error::Exception);
        }
        Value::from_raw(ctx.clone(), value)
    };
    let promise = value
        .into_promise()
        .ok_or_else(|| Exception::throw_internal(ctx, "loading a module gave no promise"))?;
    let note = Function::new(ctx.clone(), |ctx: Ctx<'js>, error: Value<'js>| {
        note_module_error(&ctx, error);
    })?;
    let then: Function<'js> = promise.get("then")?;
    then.call::<_, ()>((
        This(promise.clone()),
        Value::new_undefined(ctx.clone()),
        note,
    ))?;
    Ok(promise)
}

/// Settles the dynamic import waiting for graph `id`.
fn finish_import(ctx: &Ctx<'_>, id: u32) -> rquickjs::Result<()> {
    let Some((shared, waiting)) = ctx
        .userdata::<ModuleData<'_>>()
        .map(|d| (Rc::clone(&d.shared), d.imports.borrow_mut().remove(&id)))
    else {
        return Ok(());
    };
    let graph = shared.modules.graphs.borrow_mut().remove(&id);
    let (Some((resolve, reject)), Some(graph)) = (waiting, graph) else {
        return Ok(());
    };
    match graph.state {
        GraphState::Ready => match load_root(ctx, &graph.root) {
            Ok(namespace) => resolve.call((namespace,)),
            Err(rquickjs::Error::Exception) => reject.call((ctx.catch(),)),
            Err(e) => Err(e),
        },
        _ => reject.call((type_error(
            ctx,
            &format!(
                "Failed to fetch dynamically imported module: {}",
                graph.root
            ),
        ),)),
    }
}

/// Fires `load` or `error` at a modulepreload link whose module arrived or
/// failed.
fn finish_preload(ctx: &Ctx<'_>, id: u32, link: NodeId) -> rquickjs::Result<()> {
    let Some(shared) = shared_of(ctx) else {
        return Ok(());
    };
    let Some(graph) = shared.modules.graphs.borrow_mut().remove(&id) else {
        return Ok(());
    };
    let event = if graph.state == GraphState::Ready {
        "load"
    } else {
        "error"
    };
    // Only a listener makes the link a wrapper, which then keeps it alive;
    // the hold keeps it until the event is fired.
    let fired = events::fire(ctx, HostTarget::Node(link), event, false, false);
    shared.modules.preload_holds.borrow_mut().remove(&link);
    fired.map(drop)
}

// ---------------------------------------------------------------------------
// QuickJS hooks.

/// Compiles `source` as the module `name` (or reads it from the script
/// cache) and sets its `import.meta`. Returns null with an exception
/// pending if it does not compile.
fn compile(
    ctx: &Ctx<'_>,
    shared: &Rc<Shared>,
    name: &str,
    source: &mut String,
) -> rquickjs::Result<NonNull<qjs::JSModuleDef>> {
    let raw = ctx.as_raw().as_ptr();
    let cached = shared
        .scripts
        .as_ref()
        .and_then(|scripts| scripts.compile_module(ctx, source, name));
    let module = match cached {
        Some(module) => module?,
        None => compile_text(ctx, name, source)?,
    };
    // SAFETY: `module` was just compiled in this context and is alive in
    // its module list. `JS_GetImportMeta` returns an owned value (or an
    // exception), which `Value::from_raw` takes over.
    let meta = unsafe { Value::from_raw(ctx.clone(), qjs::JS_GetImportMeta(raw, module.as_ptr())) };
    let meta = meta.into_object().ok_or(rquickjs::Error::Exception)?;
    let base = base_url_of(shared, name);
    meta.set("url", base.as_str())?;
    let s = Rc::clone(shared);
    meta.set(
        "resolve",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'_>, specifier: Coerced<String>| -> rquickjs::Result<String> {
                let map = s.modules.import_map.borrow();
                map.resolve(&specifier, &base)
                    .map(String::from)
                    .map_err(|message| throw_type(&ctx, &message))
            },
        )?,
    )?;
    Ok(module)
}

/// Compiles `source` as the module `name`, from its text.
fn compile_text(
    ctx: &Ctx<'_>,
    name: &str,
    source: &mut String,
) -> rquickjs::Result<NonNull<qjs::JSModuleDef>> {
    let c_name = CString::new(name)?;
    let raw = ctx.as_raw().as_ptr();
    let len = source.len();
    // QuickJS wants the source NUL-terminated.
    source.push('\0');
    // SAFETY: `raw` is the live context `ctx` wraps, locked for this call.
    // The source is NUL-terminated just past `len` and outlives the call.
    // A compiled module comes back as a module value: QuickJS's list of
    // loaded modules holds it too, so the value's own reference is freed
    // once its pointer is read.
    let module = unsafe {
        let value = qjs::JS_Eval(
            raw,
            source.as_ptr().cast(),
            len as _,
            c_name.as_ptr(),
            (qjs::JS_EVAL_TYPE_MODULE | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
        );
        if qjs::JS_IsException(value) {
            None
        } else {
            let module = qjs::JS_VALUE_GET_PTR(value).cast::<qjs::JSModuleDef>();
            qjs::JS_FreeValue(raw, value);
            NonNull::new(module)
        }
    };
    source.pop();
    module.ok_or(rquickjs::Error::Exception)
}

/// QuickJS's module loader: compiles the module `name` from the module
/// map, fetching it first if it is not there.
fn load(ctx: &Ctx<'_>, name: &str) -> rquickjs::Result<NonNull<qjs::JSModuleDef>> {
    let shared = shared_of(ctx).ok_or(rquickjs::Error::Unknown)?;
    let modules = &shared.modules;
    let entry = modules.map.borrow_mut().remove(name);
    let mut fetched = match entry {
        Some(Entry::Fetched(fetched)) => fetched,
        Some(Entry::Failed(reason)) => {
            let message = format!("Failed to load module {name}: {reason}");
            modules
                .map
                .borrow_mut()
                .insert(name.to_owned(), Entry::Failed(reason));
            return Err(throw_type(ctx, &message));
        }
        // Not fetched ahead (or fetched and since unloaded): fetch it now.
        _ => {
            let result = fetch_blocking(&shared, name);
            match record(&shared, name, result) {
                Entry::Fetched(fetched) => fetched,
                failed => {
                    modules.map.borrow_mut().insert(name.to_owned(), failed);
                    return Err(throw_type(ctx, &format!("Failed to load module {name}")));
                }
            }
        }
    };
    match compile(ctx, &shared, name, &mut fetched.source) {
        Ok(module) => {
            modules
                .map
                .borrow_mut()
                .insert(name.to_owned(), Entry::Declared);
            Ok(module)
        }
        Err(e) => {
            // Kept, so importing it again throws the same error.
            modules
                .map
                .borrow_mut()
                .insert(name.to_owned(), Entry::Fetched(fetched));
            Err(e)
        }
    }
}

/// Runs `f` with the context QuickJS called back with.
///
/// # Safety
///
/// `ctx` must be the live context QuickJS passed to a callback.
unsafe fn with_callback_ctx<R>(ctx: *mut qjs::JSContext, f: impl FnOnce(Ctx<'_>) -> R) -> R {
    // SAFETY: the caller guarantees `ctx` is live and non-null.
    let ctx = unsafe { Ctx::from_raw(NonNull::new_unchecked(ctx)) };
    f(ctx)
}

unsafe extern "C" fn normalize_callback(
    ctx: *mut qjs::JSContext,
    base: *const c_char,
    name: *const c_char,
    _opaque: *mut c_void,
) -> *mut c_char {
    // SAFETY: QuickJS passes its live context and NUL-terminated strings
    // that outlive the call.
    unsafe {
        with_callback_ctx(ctx, |ctx| {
            let base = CStr::from_ptr(base).to_string_lossy();
            let name = CStr::from_ptr(name).to_string_lossy();
            let resolved = if base.is_empty() {
                // A root load: `name` is resolved already.
                Ok(name.into_owned())
            } else {
                match shared_of(&ctx) {
                    Some(shared) => resolve(&shared, &base, &name).map(String::from),
                    None => Err(String::from("modules are not available")),
                }
            };
            match resolved {
                Ok(url) => {
                    qjs::js_strndup(ctx.as_raw().as_ptr(), url.as_ptr().cast(), url.len() as _)
                }
                Err(message) => {
                    let _ = throw_type(&ctx, &message);
                    ptr::null_mut()
                }
            }
        })
    }
}

unsafe extern "C" fn load_callback(
    ctx: *mut qjs::JSContext,
    name: *const c_char,
    _opaque: *mut c_void,
) -> *mut qjs::JSModuleDef {
    // SAFETY: QuickJS passes its live context and a NUL-terminated name
    // that outlives the call.
    unsafe {
        with_callback_ctx(ctx, |ctx| {
            let name = CStr::from_ptr(name).to_string_lossy();
            match load(&ctx, &name) {
                Ok(module) => module.as_ptr(),
                Err(rquickjs::Error::Exception) => ptr::null_mut(),
                Err(e) => {
                    let _ = throw_type(&ctx, &format!("Failed to load module {name}: {e}"));
                    ptr::null_mut()
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Host functions.

/// The global `import()` calls are renamed to: fetches the graph of the
/// module `specifier` names, relative to the calling script, then loads it.
fn dynamic_import<'js>(
    ctx: Ctx<'js>,
    shared: &Rc<Shared>,
    specifier: Value<'js>,
) -> rquickjs::Result<Promise<'js>> {
    let (promise, resolve_fn, reject_fn) = ctx.promise()?;
    // Natives push no stack frame, so level 0 is the caller.
    let referrer = ctx
        .script_or_module_name(0)
        .and_then(|atom| atom.to_string().ok())
        .unwrap_or_default();
    let specifier = match Coerced::<String>::from_js(&ctx, specifier) {
        Ok(s) => s.0,
        Err(rquickjs::Error::Exception) => {
            reject_fn.call::<_, ()>((ctx.catch(),))?;
            return Ok(promise);
        }
        Err(e) => return Err(e),
    };
    let url = match resolve(shared, &referrer, &specifier) {
        Ok(url) => url,
        Err(message) => {
            reject_fn.call::<_, ()>((type_error(&ctx, &message),))?;
            return Ok(promise);
        }
    };
    let id = start_graph(
        shared,
        String::from(url),
        ModuleCredentials::SameOrigin,
        Purpose::Import,
    );
    if let Some(data) = ctx.userdata::<ModuleData<'js>>() {
        data.imports
            .borrow_mut()
            .insert(id, (resolve_fn, reject_fn));
    }
    Ok(promise)
}

/// Handles a `<link>` that became connected: starts fetching the module a
/// `rel=modulepreload` link points at.
fn link_connected(shared: &Rc<Shared>, link: NodeId) {
    let (rel, href, cross_origin) = {
        let doc = shared.dom.doc();
        let attr = |name: &str| doc.attribute(link, name).map(str::to_owned);
        (attr("rel"), attr("href"), attr("crossorigin"))
    };
    let preload = rel.is_some_and(|rel| {
        rel.split_ascii_whitespace()
            .any(|t| t.eq_ignore_ascii_case("modulepreload"))
    });
    let Some(href) = href.filter(|_| preload) else {
        return;
    };
    if !shared.modules.preload_links.borrow_mut().insert(link) {
        return;
    }
    let Ok(url) = document_base_url(shared).join(&href) else {
        return;
    };
    let credentials = credentials_for(cross_origin.as_deref());
    shared
        .modules
        .preload_holds
        .borrow_mut()
        .insert(link, dom::HeldNode::new(&shared.dom, link));
    start_graph(
        shared,
        String::from(url),
        credentials,
        Purpose::Preload(link),
    );
}

fn credentials_for(cross_origin: Option<&str>) -> ModuleCredentials {
    match cross_origin {
        Some(v) if v.eq_ignore_ascii_case("use-credentials") => ModuleCredentials::Include,
        _ => ModuleCredentials::SameOrigin,
    }
}

/// Sets up module loading and adds the module host functions to `native`
/// as `native.modules`.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> rquickjs::Result<()> {
    let data = ModuleData {
        shared: Rc::clone(shared),
        imports: RefCell::default(),
    };
    if ctx.store_userdata(data).is_err() {
        return Err(rquickjs::Error::Unknown);
    }
    // SAFETY: the runtime is alive while `ctx` is. The callbacks get their
    // state from the context's userdata, not from the opaque pointer.
    unsafe {
        qjs::JS_SetModuleLoaderFunc(
            qjs::JS_GetRuntime(ctx.as_raw().as_ptr()),
            Some(normalize_callback),
            Some(load_callback),
            ptr::null_mut(),
        );
    }

    let modules = Object::new(ctx.clone())?;
    modules.set("importName", DYNAMIC_IMPORT)?;
    let s = Rc::clone(shared);
    modules.set(
        "dynamicImport",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, specifier: Value<'js>, _options: Opt<Value<'js>>| {
                dynamic_import(ctx, &s, specifier)
            },
        )?,
    )?;
    modules.set(
        "rewrite",
        Function::new(ctx.clone(), |source: String| {
            rewrite_classic(&source).into_owned()
        })?,
    )?;
    let s = Rc::clone(shared);
    modules.set(
        "linkConnected",
        Function::new(ctx.clone(), move |node: dom::Node<'js>| {
            if let Some(link) = node.borrow().id() {
                link_connected(&s, link);
            }
        })?,
    )?;
    native.set("modules", modules)
}

// ---------------------------------------------------------------------------
// The host's API.

impl JsRuntime {
    /// Starts fetching the module graph rooted at `url` (an absolute URL),
    /// for a module script the host will run with
    /// [`run_module_graph`](Self::run_module_graph). The fetches go through
    /// the runtime's session as script requests.
    pub fn fetch_module_graph(&self, url: &str, credentials: ModuleCredentials) -> ModuleGraph {
        ModuleGraph(start_graph(
            self.shared(),
            url.to_owned(),
            credentials,
            Purpose::Host,
        ))
    }

    /// Starts fetching the imports of an inline module script, whose base
    /// URL (for its imports and `import.meta.url`) is `base_url`.
    pub fn fetch_inline_module_graph(
        &self,
        source: &str,
        base_url: &str,
        credentials: ModuleCredentials,
    ) -> ModuleGraph {
        let shared = self.shared();
        let modules = &shared.modules;
        let n = modules.inline_count.get() + 1;
        modules.inline_count.set(n);
        let base = Url::parse(base_url).unwrap_or_else(|_| document_base_url(shared));
        let mut name = base.clone();
        name.set_fragment(Some(&format!("inline-module-{n}")));
        let name = String::from(name);
        let (source, imports) = prepare(source.to_owned());
        let loaded = Loaded {
            response_url: base,
            source,
            imports,
        };
        // Its base differs from its name, so `record` notes it.
        let entry = record(shared, &name, Ok(loaded));
        modules.map.borrow_mut().insert(name.clone(), entry);
        ModuleGraph(start_graph(shared, name, credentials, Purpose::Host))
    }

    /// Whether a module graph has loaded.
    pub fn module_graph_status(&self, graph: ModuleGraph) -> ModuleGraphStatus {
        match self.shared().modules.graphs.borrow().get(&graph.0) {
            Some(Graph {
                state: GraphState::Loading,
                ..
            }) => ModuleGraphStatus::Loading,
            Some(Graph {
                state: GraphState::Ready,
                ..
            }) => ModuleGraphStatus::Ready,
            _ => ModuleGraphStatus::Failed,
        }
    }

    /// Waits until a module graph has settled since the last call, or
    /// returns at once if one has.
    pub async fn module_graph_settled(&self) {
        self.shared().modules.settled.notified().await;
    }

    /// Runs a module graph that has loaded, as a task followed by a
    /// microtask checkpoint: links it and evaluates its modules that have
    /// not run yet. An exception a module throws (or a rejection of its
    /// top-level `await`) is reported like an uncaught exception.
    ///
    /// Returns `false`, running nothing, if the graph failed to load (or
    /// is still loading); the host fires `error` at the script element.
    pub fn run_module_graph(&self, graph: ModuleGraph) -> bool {
        let shared = self.shared();
        let Some(graph) = shared.modules.graphs.borrow_mut().remove(&graph.0) else {
            return false;
        };
        if graph.state != GraphState::Ready {
            return false;
        }
        let _ = self.run_task(|ctx| {
            let promise = load_root(ctx, &graph.root)?;
            let report = reporter(ctx, Rc::clone(shared))?;
            let then: Function<'_> = promise.get("then")?;
            then.call::<_, ()>((This(promise), Value::new_undefined(ctx.clone()), report))
        });
        self.perform_microtask_checkpoint();
        true
    }

    /// Registers an import map from an inline `<script type=importmap>`,
    /// whose base URL is `base_url`. Its entries are added to those of
    /// earlier maps, which win where both have a specifier. JSON that does
    /// not parse is reported like an uncaught exception; entries that
    /// cannot be used are logged as warnings.
    pub fn register_import_map(&self, json: &str, base_url: &str) {
        let shared = self.shared();
        let base = Url::parse(base_url).unwrap_or_else(|_| document_base_url(shared));
        let mut raw = RawImportMap::default();
        let parsed = self.run_task(|ctx| {
            let value = ctx.json_parse(json)?;
            let Some(object) = value.as_object().filter(|o| !o.is_array()) else {
                return Err(throw_type(ctx, "An import map must be a JSON object."));
            };
            if let Some(imports) = object.get::<_, Option<Object<'_>>>("imports")? {
                raw.imports = entries(&imports)?;
            }
            if let Some(scopes) = object.get::<_, Option<Object<'_>>>("scopes")? {
                for scope in scopes.props::<String, Value<'_>>() {
                    let (prefix, value) = scope?;
                    let Some(entries_object) = value.into_object() else {
                        return Err(throw_type(ctx, "Import map scopes must be objects."));
                    };
                    raw.scopes.push((prefix, entries(&entries_object)?));
                }
            }
            Ok(())
        });
        self.perform_microtask_checkpoint();
        if parsed.is_err() {
            return;
        }
        let mut warnings = Vec::new();
        let map = ImportMap::parse(raw, &base, &mut warnings);
        for warning in warnings {
            log(shared, ConsoleLevel::Warn, warning);
        }
        shared.modules.import_map.borrow_mut().merge(map);
    }

    /// Tells the runtime that the parser inserted a `<link>` element: one
    /// with `rel=modulepreload` starts fetching its module, and gets a
    /// `load` or `error` event when it is done. Links that scripts insert
    /// are handled without this.
    pub fn link_inserted(&self, link: NodeId) {
        link_connected(self.shared(), link);
    }
}

/// A function that reports the exception it is called with as uncaught.
fn reporter<'js>(ctx: &Ctx<'js>, shared: Rc<Shared>) -> rquickjs::Result<Function<'js>> {
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, error: Value<'js>| {
        shared.report_exception(&ctx, error);
    })
}

/// The entries of an import map's `imports` (or one scope), in order.
/// Values that are not strings are `None`.
fn entries(object: &Object<'_>) -> rquickjs::Result<RawEntries> {
    object
        .props::<String, Value<'_>>()
        .map(|prop| {
            let (key, value) = prop?;
            let value = value.as_string().map(|s| s.to_string()).transpose()?;
            Ok((key, value))
        })
        .collect()
}
