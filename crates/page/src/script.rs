//! Script elements: what kind each one is, fetching external ones, and the
//! lists that decide when each runs. Module scripts share the lists with
//! classic ones; their graphs are fetched by the JS runtime.
//! <https://html.spec.whatwg.org/#prepare-the-script-element>

use std::collections::{HashMap, VecDeque};

use dasher_dom::{Document, Name, NodeId};
use dasher_js::{
    ConsoleLevel, EventTarget, HeldNode, JsRuntime, ModuleCredentials, ModuleGraph,
    ModuleGraphStatus, PendingOperation,
};
use dasher_net::{Credentials, Request, ResourceType, Session, Url};
use encoding_rs::Encoding;
use tokio::task::JoinSet;

use crate::charset;

/// What a script element holds, going by its `type` and `language`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptType {
    Classic,
    Module,
    ImportMap,
}

/// The JavaScript MIME type essences a classic script may declare.
const CLASSIC_TYPES: &[&str] = &[
    "text/javascript",
    "application/javascript",
    "text/ecmascript",
    "application/ecmascript",
    "application/x-javascript",
    "application/x-ecmascript",
    "text/x-javascript",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
];

/// The script's type, or `None` for a data block, which never runs.
pub(crate) fn script_type(type_attr: Option<&str>, language: Option<&str>) -> Option<ScriptType> {
    let declared = match (type_attr, language) {
        (Some(t), _) => t.trim().to_ascii_lowercase(),
        (None, None | Some("")) => return Some(ScriptType::Classic),
        (None, Some(language)) => format!("text/{}", language.trim().to_ascii_lowercase()),
    };
    match declared.as_str() {
        "" => Some(ScriptType::Classic),
        "module" => Some(ScriptType::Module),
        "importmap" => Some(ScriptType::ImportMap),
        t if CLASSIC_TYPES.contains(&t) => Some(ScriptType::Classic),
        _ => None,
    }
}

/// A script element's attributes, as preparing it needs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScriptElement {
    pub(crate) script_type: Option<ScriptType>,
    pub(crate) src: Option<String>,
    pub(crate) is_async: bool,
    pub(crate) defer: bool,
    pub(crate) no_module: bool,
    pub(crate) charset: Option<String>,
    pub(crate) cross_origin: Option<String>,
    /// The inline source text.
    pub(crate) text: String,
}

impl ScriptElement {
    /// Whether the script runs at all: data blocks never do, and
    /// `nomodule` classic scripts are for browsers without modules.
    pub(crate) fn runs(&self) -> bool {
        match self.script_type {
            Some(ScriptType::Classic) => !self.no_module,
            Some(ScriptType::Module | ScriptType::ImportMap) => true,
            None => false,
        }
    }

    /// The credentials mode module requests use, from `crossorigin`.
    pub(crate) fn module_credentials(&self) -> ModuleCredentials {
        match self.cross_origin.as_deref() {
            Some(v) if v.eq_ignore_ascii_case("use-credentials") => ModuleCredentials::Include,
            _ => ModuleCredentials::SameOrigin,
        }
    }

    pub(crate) fn read(doc: &Document, id: NodeId) -> Self {
        let attr = |name: &str| doc.attribute(id, name);
        let text = doc
            .children(id)
            .filter_map(|c| doc.text(c).filter(|_| doc.is_text(c)))
            .collect();
        Self {
            script_type: script_type(attr("type"), attr("language")),
            src: attr("src").map(str::to_owned),
            is_async: doc.has_attribute(id, "async"),
            defer: doc.has_attribute(id, "defer"),
            no_module: doc.has_attribute(id, "nomodule"),
            charset: attr("charset").map(str::to_owned),
            cross_origin: attr("crossorigin").map(str::to_owned),
            text,
        }
    }
}

/// The document's base URL: the first `<base href>`, resolved against the
/// document URL, or the document URL itself.
/// <https://html.spec.whatwg.org/#document-base-url>
pub(crate) fn base_url(doc: &Document, document_url: &Url) -> Url {
    doc.descendants(doc.root())
        .find(|&n| doc.is_html(n, Name::BASE) && doc.attribute_named(n, Name::HREF).is_some())
        .and_then(|n| doc.attribute_named(n, Name::HREF))
        .and_then(|href| document_url.join(href).ok())
        .unwrap_or_else(|| document_url.clone())
}

/// Identifies a script being fetched or waiting to run.
pub(crate) type ScriptId = u32;

/// Where a fetched script waits for its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// Parser-inserted without `async` or `defer`: the parser waits for it.
    Blocking,
    /// Parser-inserted with `defer`: runs after parsing, in document order.
    Deferred,
    /// `async`, or inserted by script and non-blocking: runs as soon as it
    /// has loaded.
    Asap,
    /// Inserted by script with `async` turned off: runs as soon as it and
    /// every such script inserted before it have loaded.
    Ordered,
}

/// What came of fetching a script.
#[derive(Debug)]
enum Fetched {
    /// The decoded source.
    Ok(String),
    /// Why it failed, for the console.
    Failed(String),
}

/// A script waiting for its turn.
#[derive(Debug)]
enum Source {
    /// A classic script at this URL, and its source once fetched.
    Classic(Url, Option<Fetched>),
    /// A module script's graph, which the runtime fetches. `external` is
    /// whether the script has a `src` (only those fire `load`).
    Module { graph: ModuleGraph, external: bool },
}

#[derive(Debug)]
struct Entry {
    element: NodeId,
    source: Source,
    /// Keeps the event loop busy until the script has run.
    _operation: PendingOperation,
    /// Keeps the element from being freed until the script has run, even
    /// if script removes it meanwhile (it still runs then, as in browsers).
    _hold: HeldNode,
}

/// External scripts in flight and the lists that order them.
#[derive(Debug, Default)]
pub(crate) struct Scripts {
    next_id: ScriptId,
    entries: HashMap<ScriptId, Entry>,
    fetches: JoinSet<(ScriptId, Fetched)>,
    asap: Vec<ScriptId>,
    in_order: VecDeque<ScriptId>,
    deferred: VecDeque<ScriptId>,
}

/// How to fetch one script.
pub(crate) struct FetchParams {
    pub(crate) url: Url,
    pub(crate) referrer: Url,
    pub(crate) credentials: Credentials,
    /// The encoding to decode with when the response names none.
    pub(crate) fallback: &'static Encoding,
}

impl FetchParams {
    /// The parameters for `element`'s script at `url`.
    pub(crate) fn new(
        element: &ScriptElement,
        url: Url,
        document_url: &Url,
        document_encoding: &'static Encoding,
    ) -> Self {
        let credentials = match element.cross_origin.as_deref() {
            None => Credentials::Include,
            Some(v) if v.eq_ignore_ascii_case("use-credentials") => Credentials::Include,
            Some(_) => Credentials::SameOrigin(document_url.origin()),
        };
        let fallback = element
            .charset
            .as_deref()
            .and_then(|label| Encoding::for_label(label.trim().as_bytes()))
            .unwrap_or(document_encoding);
        Self {
            url,
            referrer: document_url.clone(),
            credentials,
            fallback,
        }
    }
}

impl Scripts {
    /// Starts fetching a script and files it under `order`.
    pub(crate) fn fetch(
        &mut self,
        session: &Session,
        element: HeldNode,
        params: FetchParams,
        order: Order,
        operation: PendingOperation,
    ) -> ScriptId {
        let id = self.next_id;
        let session = session.clone();
        let FetchParams {
            url,
            referrer,
            credentials,
            fallback,
        } = params;
        let request = Request::get(url.clone(), ResourceType::Script)
            .with_referrer(referrer)
            .with_credentials(credentials);
        self.fetches.spawn(async move {
            let fetched = match fetch(&session, request, fallback).await {
                Ok(source) => Fetched::Ok(source),
                Err(reason) => Fetched::Failed(reason),
            };
            (id, fetched)
        });
        self.add(element, Source::Classic(url, None), order, operation)
    }

    /// Files a module script whose graph the runtime is fetching under
    /// `order`.
    pub(crate) fn module(
        &mut self,
        element: HeldNode,
        graph: ModuleGraph,
        external: bool,
        order: Order,
        operation: PendingOperation,
    ) -> ScriptId {
        self.add(
            element,
            Source::Module { graph, external },
            order,
            operation,
        )
    }

    fn add(
        &mut self,
        element: HeldNode,
        source: Source,
        order: Order,
        operation: PendingOperation,
    ) -> ScriptId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.insert(
            id,
            Entry {
                element: element.node(),
                source,
                _operation: operation,
                _hold: element,
            },
        );
        match order {
            Order::Blocking => {}
            Order::Deferred => self.deferred.push_back(id),
            Order::Asap => self.asap.push(id),
            Order::Ordered => self.in_order.push_back(id),
        }
        id
    }

    /// Whether any fetch is still in progress.
    pub(crate) fn fetching(&self) -> bool {
        !self.fetches.is_empty()
    }

    /// Waits for the next fetch to finish and records its result. Returns
    /// at once if nothing is being fetched.
    pub(crate) async fn next_fetched(&mut self) {
        let Some(joined) = self.fetches.join_next().await else {
            return;
        };
        // A fetch task only ends early if it is aborted, which only happens
        // when the whole set is dropped.
        if let Ok((id, fetched)) = joined
            && let Some(Entry {
                source: Source::Classic(_, slot),
                ..
            }) = self.entries.get_mut(&id)
        {
            *slot = Some(fetched);
        }
    }

    /// Whether script `id` has loaded (or failed to) and can run.
    pub(crate) fn is_ready(&self, js: &JsRuntime, id: ScriptId) -> bool {
        self.entries.get(&id).is_none_or(|e| match &e.source {
            Source::Classic(_, fetched) => fetched.is_some(),
            Source::Module { graph, .. } => {
                js.module_graph_status(*graph) != ModuleGraphStatus::Loading
            }
        })
    }

    /// The next deferred script, which runs once it is ready.
    pub(crate) fn next_deferred(&mut self) -> Option<ScriptId> {
        self.deferred.pop_front()
    }

    /// Whether async and in-order scripts are still waiting to run.
    pub(crate) fn has_pending_asap(&self) -> bool {
        !self.asap.is_empty() || !self.in_order.is_empty()
    }

    /// Runs the async and in-order scripts whose turn has come. Returns
    /// whether any ran.
    pub(crate) fn run_ready(&mut self, js: &JsRuntime) -> bool {
        let mut ran = false;
        while let Some(i) = self.asap.iter().position(|&id| self.is_ready(js, id)) {
            let id = self.asap.remove(i);
            self.execute(js, id);
            ran = true;
        }
        while let Some(&id) = self.in_order.front() {
            if !self.is_ready(js, id) {
                break;
            }
            self.in_order.pop_front();
            self.execute(js, id);
            ran = true;
        }
        ran
    }

    /// Runs script `id`, which must be ready, then fires `load` at its
    /// element if it is external; or, if it failed to load, logs why and
    /// fires `error`.
    pub(crate) fn execute(&mut self, js: &JsRuntime, id: ScriptId) {
        let Some(entry) = self.entries.remove(&id) else {
            return;
        };
        let target = EventTarget::Node(entry.element);
        match entry.source {
            Source::Classic(url, Some(Fetched::Ok(source))) => {
                let _ = js.run_script_element(entry.element, &source, url.as_str());
                js.fire_event(target, "load", false, false);
            }
            Source::Classic(url, Some(Fetched::Failed(reason))) => {
                js.log(
                    ConsoleLevel::Error,
                    format!("Failed to load script {url}: {reason}"),
                );
                js.fire_event(target, "error", false, false);
            }
            Source::Classic(_, None) => debug_assert!(false, "script {id} ran before it loaded"),
            // The runtime logged why a module graph failed.
            Source::Module { graph, external } => {
                if !js.run_module_graph(graph) {
                    js.fire_event(target, "error", false, false);
                } else if external {
                    js.fire_event(target, "load", false, false);
                }
            }
        }
    }
}

/// Fetches and decodes a classic script. Responses that are not `ok` fail,
/// as in browsers, rather than running an error page as script. A `data:`
/// URL is decoded on the spot, as browsers do (Instagram loads its
/// bootstrap scripts that way).
async fn fetch(
    session: &Session,
    request: Request,
    fallback: &'static Encoding,
) -> Result<String, String> {
    if request.url.scheme() == "data" {
        let (mime, body) = dasher_js::decode_data_url(request.url.as_str())
            .ok_or_else(|| String::from("invalid data: URL"))?;
        let encoding = charset::from_content_type(&mime).unwrap_or(fallback);
        return Ok(charset::decode_script(&body, encoding));
    }
    let response = session.send(request).await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP status {}", status.as_u16()));
    }
    let encoding = response
        .content_type()
        .and_then(charset::from_content_type)
        .unwrap_or(fallback);
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    Ok(charset::decode_script(&bytes, encoding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_types() {
        use ScriptType::*;
        let cases = [
            (None, None, Some(Classic)),
            (Some(""), None, Some(Classic)),
            (Some(" Text/JavaScript "), None, Some(Classic)),
            (Some("application/x-javascript"), None, Some(Classic)),
            (Some("text/javascript; charset=utf-8"), None, None),
            (Some("module"), None, Some(Module)),
            (Some("importmap"), None, Some(ImportMap)),
            (Some("application/json"), None, None),
            (Some("text/x-template"), None, None),
            (None, Some(""), Some(Classic)),
            (None, Some("JavaScript"), Some(Classic)),
            (None, Some("javascript1.2"), Some(Classic)),
            (None, Some("vbscript"), None),
            // `type` wins over `language`.
            (Some("module"), Some("javascript"), Some(Module)),
        ];
        for (t, language, expected) in cases {
            assert_eq!(script_type(t, language), expected, "{t:?} {language:?}");
        }
    }

    #[test]
    fn reads_script_elements() {
        let doc = Document::parse(
            "<script src=a.js async defer nomodule charset=latin1 crossorigin>x</script>\
             <script>one<!-- two --></script>",
        );
        let scripts: Vec<NodeId> = doc
            .descendants(doc.root())
            .filter(|&n| doc.is_html(n, Name::SCRIPT))
            .collect();
        let a = ScriptElement::read(&doc, scripts[0]);
        assert_eq!(a.script_type, Some(ScriptType::Classic));
        assert_eq!(a.src.as_deref(), Some("a.js"));
        assert!(a.is_async && a.defer && a.no_module);
        assert_eq!(a.charset.as_deref(), Some("latin1"));
        assert_eq!(a.cross_origin.as_deref(), Some(""));
        let b = ScriptElement::read(&doc, scripts[1]);
        assert_eq!(b.src, None);
        assert_eq!(b.text, "one<!-- two -->");
    }

    #[test]
    fn base_urls() {
        let page = Url::parse("https://example.com/a/b.html").unwrap();
        let doc = Document::parse("<base target=_top><base href=/root/><base href=/other/>");
        assert_eq!(base_url(&doc, &page).as_str(), "https://example.com/root/");
        let doc = Document::parse("<p>");
        assert_eq!(base_url(&doc, &page), page);
        let doc = Document::parse("<base href='https://cdn.example.net/x/'>");
        assert_eq!(base_url(&doc, &page).as_str(), "https://cdn.example.net/x/");
    }

    #[test]
    fn fetch_params() {
        let page = Url::parse("https://example.com/").unwrap();
        let script = Url::parse("https://cdn.example.net/a.js").unwrap();
        let doc = Document::parse("<script crossorigin=anonymous charset=windows-1251></script>");
        let el = doc
            .descendants(doc.root())
            .find(|&n| doc.is_html(n, Name::SCRIPT))
            .unwrap();
        let params = FetchParams::new(
            &ScriptElement::read(&doc, el),
            script,
            &page,
            encoding_rs::UTF_8,
        );
        assert_eq!(params.credentials, Credentials::SameOrigin(page.origin()));
        assert_eq!(params.fallback, encoding_rs::WINDOWS_1251);
        assert_eq!(params.referrer, page);
    }
}
