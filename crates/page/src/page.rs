//! A loaded page and the loading pipeline that builds it.

use std::fmt;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dasher_dom::{Document, DocumentParser, Name, NodeId, ParseOptions, ParseStep, ShadowRoots};
use dasher_js::{
    DocumentInfo, DocumentWrite, EventTarget, JsRuntime, PendingOperation, ReadyState,
    ResourceLimit, RunLimits, RunOutcome, RuntimeOptions, ScriptError, WindowOptions,
};
use dasher_net::{HeaderMap, Request, ResourceType, Response, Session, StatusCode, Url};
use encoding_rs::Encoding;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::charset;
use crate::script::{FetchParams, Order, ScriptElement, ScriptId, ScriptType, Scripts, base_url};

/// Default for [`PageOptions::max_navigations`].
pub const DEFAULT_MAX_NAVIGATIONS: usize = 5;

/// How a [`Page`] is loaded.
#[derive(Clone, Debug)]
pub struct PageOptions {
    /// How the page's JS runtime is set up. [`WindowOptions::url`] and
    /// [`RuntimeOptions::network`] are ignored: the page sets them to the
    /// document's URL and its session.
    pub runtime: RuntimeOptions,
    /// How many script-triggered navigations (`location.href = …`,
    /// `location.replace()`, `location.reload()`) are followed. Once they
    /// are used up, further requests are dropped and the current document
    /// stays.
    pub max_navigations: usize,
    /// How long loading may take in all, navigations included. When it
    /// runs out, loading stops where it is and the page is returned as it
    /// stands (see [`Page::timed_out`]). `None` waits as long as it takes.
    pub timeout: Option<Duration>,
}

impl Default for PageOptions {
    fn default() -> Self {
        Self {
            runtime: RuntimeOptions::default(),
            max_navigations: DEFAULT_MAX_NAVIGATIONS,
            timeout: Some(Duration::from_secs(30)),
        }
    }
}

/// Why a page could not be loaded.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The URL is not an absolute `http` or `https` URL.
    InvalidUrl(String),
    /// The document could not be fetched.
    Network(dasher_net::Error),
    /// The time limit ran out before the document's response arrived.
    Timeout,
    /// The page's JS runtime could not be created.
    Runtime(ScriptError),
    /// The page went over a resource limit while loading: it filled its
    /// JS heap or built too many DOM nodes (see
    /// [`RuntimeOptions::memory_limit`] and
    /// [`RuntimeOptions::max_dom_nodes`]). The page was dropped.
    LimitExceeded(ResourceLimit),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(url) => write!(f, "cannot load {url:?}: not an http(s) URL"),
            Self::Network(e) => write!(f, "loading the document failed: {e}"),
            Self::Timeout => f.write_str("timed out waiting for the document"),
            Self::Runtime(e) => write!(f, "creating the JS runtime failed: {e}"),
            Self::LimitExceeded(limit) => write!(f, "loading stopped: {limit}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(e) => Some(e),
            Self::Runtime(e) => Some(e),
            _ => None,
        }
    }
}

/// Why loading stopped early.
pub(crate) enum Stop {
    /// The page asked to go elsewhere, and may. The request is left
    /// pending in the runtime.
    Navigate,
    /// The time limit ran out or the runtime was interrupted.
    Halted,
    /// The page went over a resource limit and can run no more.
    Limit(ResourceLimit),
}

/// A live page: a document, its JS runtime and the session they load
/// through.
///
/// [`Page::load`] fetches a document and runs it the way a browser does,
/// returning once the `load` event has fired. The page stays live
/// afterwards: [`run`](Self::run) keeps its event loop going, still
/// loading the scripts it inserts. Dropping the page frees its document and
/// every JS object, and cancels its requests.
pub struct Page {
    // Field order is drop order: scripts in flight go before the runtime.
    scripts: Scripts,
    pub(crate) js: JsRuntime,
    session: Session,
    options: PageOptions,
    pub(crate) deadline: Option<Instant>,
    navigations_left: usize,
    status: StatusCode,
    headers: HeaderMap,
    encoding: &'static Encoding,
    timed_out: bool,
}

impl fmt::Debug for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("url", &self.js.url())
            .field("status", &self.status)
            .field("encoding", &self.encoding.name())
            .field("ready_state", &self.js.ready_state())
            .finish_non_exhaustive()
    }
}

impl Page {
    /// Loads `url` through `session` and runs the page until its `load`
    /// event has fired, following the navigations its scripts request.
    ///
    /// Must run inside a Tokio runtime, on the thread that will use the
    /// page (for example in a `LocalSet` or a current-thread runtime).
    ///
    /// Fails only if no document could be had at all. Non-2xx responses
    /// are loaded like any other, as browsers show error pages; check
    /// [`status`](Self::status). Running out of time mid-load returns the
    /// page as far as it got.
    pub async fn load(
        session: &Session,
        url: &str,
        options: PageOptions,
    ) -> Result<Page, LoadError> {
        let url = Url::parse(url).map_err(|_| LoadError::InvalidUrl(url.to_owned()))?;
        let deadline = options.timeout.map(|t| Instant::now() + t);
        let budget = options.max_navigations;
        let mut page = Page::open(session.clone(), url, None, options, deadline, budget).await?;
        while page.navigate(deadline).await? {}
        page.clear_deadline();
        Ok(page)
    }

    /// Fetches `url` and loads it as a new page, stopping at the end of
    /// loading or at a navigation request, which is left pending.
    async fn open(
        session: Session,
        url: Url,
        referrer: Option<Url>,
        options: PageOptions,
        deadline: Option<Instant>,
        navigations_left: usize,
    ) -> Result<Page, LoadError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(LoadError::InvalidUrl(url.to_string()));
        }
        let mut request = Request::get(url, ResourceType::Document);
        if let Some(referrer) = &referrer {
            request = request.with_referrer(referrer.clone());
        }
        let response = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline.into(), session.send(request))
                .await
                .map_err(|_| LoadError::Timeout)?,
            None => session.send(request).await,
        }
        .map_err(LoadError::Network)?;

        let runtime = RuntimeOptions {
            window: WindowOptions {
                url: response.url().to_string(),
                ..options.runtime.window.clone()
            },
            network: Some(session.clone()),
            ..options.runtime.clone()
        };
        let js = JsRuntime::new(runtime).map_err(LoadError::Runtime)?;
        js.set_deadline(deadline);
        js.take_over_inserted_scripts();
        let mut page = Page {
            scripts: Scripts::default(),
            js,
            session,
            options,
            deadline,
            navigations_left,
            status: response.status(),
            headers: response.headers().clone(),
            encoding: encoding_rs::UTF_8,
            timed_out: false,
        };
        let referrer = referrer.map(String::from).unwrap_or_default();
        match page.run_load(response, referrer).await {
            // A navigation request stays pending for `navigate`.
            Ok(()) | Err(Stop::Navigate) => {}
            Err(Stop::Halted) => page.timed_out = true,
            Err(Stop::Limit(limit)) => return Err(LoadError::LimitExceeded(limit)),
        }
        page.js.set_document_write(DocumentWrite::Insert);
        page.js.hold_detached_nodes(false);
        Ok(page)
    }

    /// Follows the navigation the page asked for, if any, as
    /// [`load`](Self::load) does: the requested document replaces this one
    /// and is loaded until its `load` event, following its own navigation
    /// requests in turn, with a fresh [`PageOptions::timeout`]. Returns
    /// whether the page navigated.
    ///
    /// The new document gets a fresh JS runtime with the same options and
    /// session, so cookies and storage carry over. Navigations count
    /// against [`PageOptions::max_navigations`] across the page's whole
    /// life; requests past it are dropped. So are requests for URLs that
    /// are not `http` or `https`.
    pub async fn follow_navigation(&mut self) -> Result<bool, LoadError> {
        let deadline = self.options.timeout.map(|t| Instant::now() + t);
        let mut navigated = false;
        while self.navigate(deadline).await? {
            navigated = true;
        }
        self.clear_deadline();
        Ok(navigated)
    }

    /// Lifts the loading time limit, so the host can go on using the page.
    pub(crate) fn clear_deadline(&mut self) {
        self.deadline = None;
        self.js.set_deadline(None);
    }

    /// Loads the requested document in place of this one, if there is a
    /// request and the budget allows it.
    async fn navigate(&mut self, deadline: Option<Instant>) -> Result<bool, LoadError> {
        let Some(request) = self.js.take_navigation_request() else {
            return Ok(false);
        };
        if self.navigations_left == 0 || self.timed_out {
            return Ok(false);
        }
        let Ok(url) = Url::parse(&request.url) else {
            return Ok(false);
        };
        if !matches!(url.scheme(), "http" | "https") {
            // javascript:, about:, mailto: and the like: stay put.
            return Ok(false);
        }
        let referrer = Url::parse(&self.js.url()).ok();
        let next = Page::open(
            self.session.clone(),
            url,
            referrer,
            self.options.clone(),
            deadline,
            self.navigations_left - 1,
        )
        .await?;
        *self = next;
        Ok(true)
    }

    // -----------------------------------------------------------------
    // Accessors.

    /// The page's JS runtime, which also holds its document.
    pub fn js(&self) -> &JsRuntime {
        &self.js
    }

    /// The session the page loads through.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The document's current URL: the response URL after redirects, then
    /// wherever `pushState` and fragment changes have taken it.
    pub fn url(&self) -> String {
        self.js.url()
    }

    /// The document response's status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The document response's headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The encoding the document was decoded with, e.g. `"UTF-8"`.
    pub fn encoding(&self) -> &'static str {
        self.encoding.name()
    }

    /// `document.readyState`.
    pub fn ready_state(&self) -> ReadyState {
        self.js.ready_state()
    }

    /// Whether loading stopped before the `load` event because the time
    /// limit ran out (or the runtime was interrupted).
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    /// The document serialized as HTML, doctype included. Shadow trees
    /// (open and closed) are included as declarative shadow DOM: each a
    /// `<template shadowrootmode>` first in its host, as
    /// `getHTML({ serializableShadowRoots: true })` writes them. A browser
    /// loading this HTML builds the same shadow trees.
    pub fn html(&self) -> String {
        self.js.with_document(|doc| {
            doc.children(doc.root())
                .map(|n| doc.outer_html_with(n, ShadowRoots::All))
                .collect()
        })
    }

    /// Runs the page's event loop, as [`JsRuntime::run`] does, while still
    /// loading and running the scripts the page inserts. A navigation
    /// request ends the run with [`RunOutcome::NavigationRequested`]; call
    /// [`follow_navigation`](Self::follow_navigation) to follow it.
    pub async fn run(&mut self, limits: RunLimits) -> RunOutcome {
        let mut wait = Wait::new(None);
        match self.spin(&mut wait, Until::Run(limits)).await {
            Ok(_) => wait.outcome.unwrap_or(RunOutcome::Idle),
            Err(Stop::Navigate) => RunOutcome::NavigationRequested,
            Err(Stop::Halted) => RunOutcome::DeadlineExceeded,
            Err(Stop::Limit(limit)) => RunOutcome::LimitExceeded(limit),
        }
    }

    // -----------------------------------------------------------------
    // Loading.
    // https://html.spec.whatwg.org/#parsing-main-inhead and "the end":
    // https://html.spec.whatwg.org/#the-end

    async fn run_load(&mut self, response: Response, referrer: String) -> Result<(), Stop> {
        // Until the load event, the loop always has work in progress: it
        // neither reports itself idle nor skips ahead in virtual time.
        let _loading = self.js.task_sender().begin_operation();
        let content_type = response.content_type().map(str::to_owned);
        let plain_text = content_type.as_deref().is_some_and(is_text_type);
        let body = BodyReader::start(response);
        let mut wait = Wait::new(Some(body));

        self.js.set_document(Document::new());
        // The parser holds ids of open elements across script runs.
        self.js.hold_detached_nodes(true);
        self.js.set_document_write(DocumentWrite::Ignore);
        self.js.set_ready_state(ReadyState::Loading);

        // Sniff the encoding from the first bytes.
        let mut head = Vec::new();
        let mut eof = false;
        while head.len() < charset::PRESCAN_LEN && !eof {
            match self.spin(&mut wait, Until::Body).await? {
                Some(chunk) => head.extend_from_slice(&chunk),
                None => eof = true,
            }
        }
        self.encoding = charset::sniff(content_type.as_deref(), &head, eof);
        let mime = content_type
            .as_deref()
            .and_then(|t| t.split(';').next())
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| String::from("text/html"));
        self.js.set_document_info(&DocumentInfo {
            character_set: self.encoding.name().to_owned(),
            content_type: mime,
            referrer,
        });

        let mut decoder = charset::decoder(self.encoding);
        let mut parser = self.js.with_document_mut(|doc| {
            if plain_text {
                DocumentParser::new_plain_text(doc)
            } else {
                DocumentParser::new(doc, ParseOptions::default())
            }
        });
        parser.push(&charset::decode_chunk(&mut decoder, &head, eof));
        drop(head);
        loop {
            let step = self.js.with_document_mut(|doc| parser.resume(doc));
            // Parsing alone can build too many nodes.
            if let Some(limit) = self.js.limit_exceeded() {
                return Err(Stop::Limit(limit));
            }
            // Browsers construct custom elements as they parse them, so
            // they are upgraded before the next script runs.
            self.js.upgrade_custom_elements();
            match step {
                ParseStep::Script(script) => {
                    self.parser_script(script, &mut parser, &mut wait).await?
                }
                ParseStep::NeedInput if eof => break,
                ParseStep::NeedInput => {
                    self.preload_links(false);
                    let chunk = self.spin(&mut wait, Until::Body).await?;
                    eof = chunk.is_none();
                    let bytes = chunk.unwrap_or_default();
                    parser.push(&charset::decode_chunk(&mut decoder, &bytes, eof));
                }
            }
        }
        self.js.with_document_mut(|doc| parser.finish(doc));
        drop(parser);
        self.js.upgrade_custom_elements();
        self.js.hold_detached_nodes(false);
        self.preload_links(true);

        self.js.set_ready_state(ReadyState::Interactive);
        self.check_navigation()?;
        while let Some(id) = self.scripts.next_deferred() {
            self.spin(&mut wait, Until::Script(id)).await?;
            self.scripts.execute(&self.js, id);
            self.check_navigation()?;
        }
        self.js
            .fire_event(EventTarget::Document, "DOMContentLoaded", true, false);
        self.check_navigation()?;
        self.spin(&mut wait, Until::AsapScriptsDone).await?;

        self.js.set_ready_state(ReadyState::Complete);
        self.js
            .fire_event(EventTarget::Window, "load", false, false);
        self.js
            .fire_event(EventTarget::Window, "pageshow", false, false);
        self.js.set_document_write(DocumentWrite::Insert);
        self.check_navigation()
    }

    /// Prepares a script element the parser just closed and, if the parser
    /// must wait for it, runs it.
    async fn parser_script(
        &mut self,
        script: NodeId,
        parser: &mut DocumentParser,
        wait: &mut Wait,
    ) -> Result<(), Stop> {
        // Parsing alone does not notice the time limit.
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Stop::Halted);
        }
        let document_url = self.document_url();
        let (element, base) = self.js.with_document_mut(|doc| {
            let element = ScriptElement::read(doc, script);
            // Scripts in template contents never run.
            let runs =
                doc.is_connected(script) && element.runs() && !doc.script_already_started(script);
            if runs {
                doc.set_script_already_started(script);
            }
            (runs.then_some(element), base_url(doc, &document_url))
        });
        self.preload_links(false);
        let Some(element) = element else {
            return Ok(());
        };
        match element.script_type {
            Some(ScriptType::Module) => {
                // Parser-inserted module scripts are deferred unless async.
                let order = if element.is_async {
                    Order::Asap
                } else {
                    Order::Deferred
                };
                let operation = self.js.task_sender().begin_operation();
                self.prepare_module(script, &element, &base, order, operation);
                return self.check_navigation();
            }
            Some(ScriptType::ImportMap) => {
                self.prepare_import_map(script, &element, &base);
                return self.check_navigation();
            }
            _ => {}
        }
        let Some(src) = element.src.as_deref() else {
            self.run_parser_script(script, &element.text, document_url.as_str(), parser);
            return self.check_navigation();
        };
        let Some(url) = self.script_url(script, src, &base) else {
            return self.check_navigation();
        };
        let params = FetchParams::new(&element, url, &document_url, self.encoding);
        let order = if element.is_async {
            Order::Asap
        } else if element.defer {
            Order::Deferred
        } else {
            Order::Blocking
        };
        let operation = self.js.task_sender().begin_operation();
        let id = self
            .scripts
            .fetch(&self.session, script, params, order, operation);
        if order == Order::Blocking {
            self.spin(wait, Until::Script(id)).await?;
            self.js.set_document_write(DocumentWrite::Capture);
            self.scripts.execute(&self.js, id);
            self.take_written(parser);
        }
        self.check_navigation()
    }

    /// Runs an inline script for the parser, feeding what it writes back
    /// in at the insertion point.
    fn run_parser_script(
        &self,
        script: NodeId,
        source: &str,
        url: &str,
        parser: &mut DocumentParser,
    ) {
        self.js.set_document_write(DocumentWrite::Capture);
        let _ = self.js.run_script_element(script, source, url);
        self.take_written(parser);
    }

    fn take_written(&self, parser: &mut DocumentParser) {
        self.js.set_document_write(DocumentWrite::Ignore);
        parser.insert(&self.js.take_document_write());
    }

    /// Resolves a script's `src` against the base URL. If it is empty or
    /// does not parse, fires `error` at the element and returns `None`.
    fn script_url(&self, element: NodeId, src: &str, base: &Url) -> Option<Url> {
        let url = (!src.is_empty()).then(|| base.join(src).ok()).flatten();
        if url.is_none() {
            self.js.log(
                dasher_js::ConsoleLevel::Error,
                format!("Script source {src:?} is not a valid URL"),
            );
            self.js
                .fire_event(EventTarget::Node(element), "error", false, false);
        }
        url
    }

    /// Starts loading a module script's graph and files it under `order`.
    fn prepare_module(
        &mut self,
        script: NodeId,
        element: &ScriptElement,
        base: &Url,
        order: Order,
        operation: PendingOperation,
    ) {
        let credentials = element.module_credentials();
        let (graph, external) = match element.src.as_deref() {
            Some(src) => {
                let Some(url) = self.script_url(script, src, base) else {
                    return;
                };
                (self.js.fetch_module_graph(url.as_str(), credentials), true)
            }
            None => {
                let graph =
                    self.js
                        .fetch_inline_module_graph(&element.text, base.as_str(), credentials);
                (graph, false)
            }
        };
        self.scripts
            .module(script, graph, external, order, operation);
    }

    /// Registers an inline import map. External ones are not supported, by
    /// browsers either: they get an `error` event.
    fn prepare_import_map(&self, script: NodeId, element: &ScriptElement, base: &Url) {
        if element.src.is_some() {
            self.js.log(
                dasher_js::ConsoleLevel::Error,
                String::from("External import maps are not supported"),
            );
            self.js
                .fire_event(EventTarget::Node(script), "error", false, false);
            return;
        }
        self.js.register_import_map(&element.text, base.as_str());
    }

    /// Hands the `<link rel=modulepreload>` elements the parser has inserted
    /// to the runtime, which starts fetching their modules. Those in
    /// `<head>`, where they belong, are looked for at every pause of the
    /// parser; the rest of the document only when `everywhere` is set, at
    /// the end of parsing. The runtime ignores links it has seen.
    fn preload_links(&self, everywhere: bool) {
        let links: Vec<NodeId> = self.js.with_document(|doc| {
            let root = if everywhere {
                Some(doc.root())
            } else {
                doc.head()
            };
            root.into_iter()
                .flat_map(|root| doc.descendants(root))
                .filter(|&n| {
                    doc.is_html(n, Name::LINK)
                        && doc.attribute(n, "rel").is_some_and(|rel| {
                            rel.split_ascii_whitespace()
                                .any(|t| t.eq_ignore_ascii_case("modulepreload"))
                        })
                })
                .collect()
        });
        for link in links {
            self.js.link_inserted(link);
        }
    }

    /// Prepares the scripts that scripts inserted since the last call.
    fn prepare_inserted(&mut self) {
        let inserted = self.js.take_inserted_scripts();
        if inserted.is_empty() {
            return;
        }
        let document_url = self.document_url();
        for script in inserted {
            let element = script.element;
            let (prepared, base) = self.js.with_document_mut(|doc| {
                let prepared = ScriptElement::read(doc, element);
                // Inline classic scripts ran as they were inserted.
                let runs = prepared.runs()
                    && (prepared.src.is_some()
                        || prepared.script_type != Some(ScriptType::Classic))
                    && !doc.script_already_started(element);
                if runs {
                    doc.set_script_already_started(element);
                }
                (runs.then_some(prepared), base_url(doc, &document_url))
            });
            let Some(prepared) = prepared else {
                continue;
            };
            let order = if script.non_blocking || prepared.is_async {
                Order::Asap
            } else {
                Order::Ordered
            };
            match prepared.script_type {
                Some(ScriptType::Module) => {
                    self.prepare_module(element, &prepared, &base, order, script.operation);
                    continue;
                }
                Some(ScriptType::ImportMap) => {
                    self.prepare_import_map(element, &prepared, &base);
                    continue;
                }
                _ => {}
            }
            let src = prepared.src.as_deref().unwrap_or_default();
            let Some(url) = self.script_url(element, src, &base) else {
                continue;
            };
            let params = FetchParams::new(&prepared, url, &document_url, self.encoding);
            self.scripts
                .fetch(&self.session, element, params, order, script.operation);
        }
    }

    fn document_url(&self) -> Url {
        Url::parse(&self.js.url()).expect("the runtime keeps an absolute URL")
    }

    /// Fails with [`Stop::Limit`] if the page went over a resource limit,
    /// else with [`Stop::Navigate`] if the page asked to navigate and may.
    /// A request past the navigation budget is dropped.
    pub(crate) fn check_navigation(&self) -> Result<(), Stop> {
        if let Some(limit) = self.js.limit_exceeded() {
            return Err(Stop::Limit(limit));
        }
        if self.js.navigation_request().is_none() {
            return Ok(());
        }
        if self.navigations_left > 0 {
            return Err(Stop::Navigate);
        }
        if let Some(request) = self.js.take_navigation_request() {
            self.js.log(
                dasher_js::ConsoleLevel::Warn,
                format!(
                    "Not following navigation to {}: too many navigations",
                    request.url
                ),
            );
        }
        Ok(())
    }

    /// Runs the event loop, loading and running scripts as they become
    /// ready, until `until` holds.
    ///
    /// For [`Until::Body`], returns the next chunk of the document, or
    /// `None` at its end.
    pub(crate) async fn spin(
        &mut self,
        wait: &mut Wait,
        until: Until,
    ) -> Result<Option<Bytes>, Stop> {
        let mut loop_active = true;
        loop {
            self.prepare_inserted();
            if self.scripts.run_ready(&self.js) {
                loop_active = true;
            }
            self.check_navigation()?;
            match until {
                Until::Script(id) if self.scripts.is_ready(&self.js, id) => return Ok(None),
                Until::AsapScriptsDone if !self.scripts.has_pending_asap() => return Ok(None),
                Until::Run(_) if wait.outcome.is_some() => return Ok(None),
                _ => {}
            }
            let limits = match until {
                Until::Run(limits) => limits,
                _ => RunLimits::default(),
            };
            let deadline = self.deadline;
            let Page { js, scripts, .. } = self;
            let fetching = scripts.fetching();
            let body = wait.body.as_mut();
            let reading = body.is_some() && matches!(until, Until::Body);
            tokio::select! {
                chunk = async { body.expect("checked").next().await }, if reading => {
                    return Ok(chunk);
                }
                () = scripts.next_fetched(), if fetching => {}
                () = js.script_inserted() => {}
                () = js.module_graph_settled() => {}
                outcome = js.run(limits), if loop_active => match outcome {
                    RunOutcome::NavigationRequested => {}
                    RunOutcome::DeadlineExceeded
                    | RunOutcome::Interrupted
                    | RunOutcome::LimitExceeded(_) => {
                        if let Until::Run(_) = until {
                            wait.outcome = Some(outcome);
                            return Ok(None);
                        }
                        if let RunOutcome::LimitExceeded(limit) = outcome {
                            return Err(Stop::Limit(limit));
                        }
                        return Err(Stop::Halted);
                    }
                    RunOutcome::Idle | RunOutcome::ReachedTime | RunOutcome::TaskLimit => {
                        if let Until::Run(_) = until {
                            wait.outcome = Some(outcome);
                        } else {
                            // Nothing to run until a script or chunk arrives.
                            loop_active = false;
                        }
                    }
                },
                () = sleep_until(deadline), if !loop_active => return Err(Stop::Halted),
            }
        }
    }
}

/// What [`Page::spin`] waits for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Until {
    /// The next chunk of the document.
    Body,
    /// Script `id` to have loaded.
    Script(ScriptId),
    /// The async and in-order scripts to have run.
    AsapScriptsDone,
    /// The event loop to stop, as `JsRuntime::run` with these limits does.
    Run(RunLimits),
}

/// State that outlives one [`Page::spin`].
pub(crate) struct Wait {
    body: Option<BodyReader>,
    pub(crate) outcome: Option<RunOutcome>,
}

impl Wait {
    pub(crate) fn new(body: Option<BodyReader>) -> Self {
        Self {
            body,
            outcome: None,
        }
    }
}

/// Reads the document body on its own task, so waiting for the next chunk
/// can be abandoned and resumed without losing data.
pub(crate) struct BodyReader {
    chunks: mpsc::Receiver<Bytes>,
    task: JoinHandle<()>,
}

impl BodyReader {
    fn start(mut response: Response) -> Self {
        let (sender, chunks) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            // A body cut short by an error ends the document there, as in
            // browsers.
            while let Ok(Some(chunk)) = response.chunk().await {
                if sender.send(chunk).await.is_err() {
                    break;
                }
            }
        });
        Self { chunks, task }
    }

    async fn next(&mut self) -> Option<Bytes> {
        self.chunks.recv().await
    }
}

impl Drop for BodyReader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

/// Whether a document of this MIME type is shown as plain text rather
/// than parsed as HTML.
fn is_text_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/html"
        | "text/xml"
        | "application/xhtml+xml"
        | "application/xml"
        | "image/svg+xml" => false,
        "application/json"
        | "application/javascript"
        | "application/x-javascript"
        | "application/ecmascript"
        | "application/manifest+json" => true,
        t => t.starts_with("text/") || t.ends_with("+json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_types() {
        for t in [
            "text/plain",
            "text/css; charset=utf-8",
            "application/json",
            "application/ld+json",
            "TEXT/CSV",
        ] {
            assert!(is_text_type(t), "{t}");
        }
        for t in [
            "text/html",
            "text/html; charset=utf-8",
            "application/xhtml+xml",
            "image/png",
            "application/octet-stream",
            "",
        ] {
            assert!(!is_text_type(t), "{t}");
        }
    }
}
