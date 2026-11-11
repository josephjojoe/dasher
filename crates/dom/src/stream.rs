//! Incremental document parsing, pausing at scripts.
//!
//! [`Document::parse`] builds a whole document in one go. A page loader
//! needs more: input arrives in chunks off the network, the parser has to
//! stop after each `</script>` so the script can run against the tree built
//! so far, and `document.write()` inserts markup right where the parser
//! stopped. [`DocumentParser`] keeps html5ever's tokenizer and tree builder
//! alive between steps for that.
//!
//! The parser does not own the document it builds. Each call that parses
//! borrows it, so between calls the document can live elsewhere (in the JS
//! runtime, say) and scripts can change it. Nodes the parser still refers to
//! (its stack of open elements, for instance) must not be
//! [released](Document::release) until [`DocumentParser::finish`] returns;
//! detaching them is fine, the parser then builds into a detached subtree,
//! as browsers do.
//!
//! Browsers queue a mutation record for every change the parser makes to
//! the tree. So that the host can do the same, the parser can
//! [log](DocumentParser::log_changes) its changes as [`TreeChange`]s.

use std::cell::RefCell;

use html5ever::TokenizerResult;
use html5ever::driver::Parser;
use html5ever::tendril::StrTendril;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{ParseOpts, parse_document};

use crate::document::Document;
use crate::name::Name;
use crate::node::NodeId;
use crate::parse::{ChangeLog, DocSlot, ParseOptions, Sink};

/// Where an incremental parse stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseStep {
    /// Every byte of input given so far is consumed. Push more, or call
    /// [`finish`](DocumentParser::finish) at the end of the input.
    NeedInput,
    /// The parser just closed this `<script>` element and is paused so it
    /// can be prepared and, if it should, run before parsing resumes. The
    /// element is in the tree with all of its text. Scripts in template
    /// contents pause too, though they are not connected and must not run.
    Script(NodeId),
}

/// A change a [`DocumentParser`] made to the tree, as logged by
/// [`DocumentParser::log_changes`]. Each is one the DOM standard queues a
/// mutation record for. Sibling ids are as they were right after (for an
/// insertion) or right before (for a removal) the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeChange {
    /// `node` was inserted into `parent`, between `prev` and `next`.
    Inserted {
        parent: NodeId,
        node: NodeId,
        prev: Option<NodeId>,
        next: Option<NodeId>,
    },
    /// `node` was removed from between `prev` and `next` in `parent`, as
    /// the tree builder does when it moves misnested and foster-parented
    /// content. If it was moved, an `Inserted` change follows.
    Removed {
        parent: NodeId,
        node: NodeId,
        prev: Option<NodeId>,
        next: Option<NodeId>,
    },
    /// Text was appended to the text node `node`, whose data was `old_len`
    /// bytes long before. Logged once per node until the changes are
    /// [taken](DocumentParser::take_changes), and not for text nodes
    /// inserted since then, whose text is all part of the insertion.
    TextAppended { node: NodeId, old_len: usize },
    /// The element `node` gained the attribute `local` in `namespace`, as
    /// `<html>` and `<body>` do from attributes on repeated start tags.
    AttributeAdded {
        node: NodeId,
        namespace: Name,
        local: Name,
    },
}

/// An HTML document parser that is fed input piece by piece and pauses
/// after every script element.
///
/// ```
/// use dasher_dom::{Document, DocumentParser, ParseOptions, ParseStep};
///
/// let mut doc = Document::new();
/// let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
/// parser.push("<p>one<script>document.write('<b>two</b>')</scr");
/// assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
/// parser.push("ipt><p>three");
/// let ParseStep::Script(script) = parser.resume(&mut doc) else { panic!() };
/// assert_eq!(doc.text_content(script).unwrap(), "document.write('<b>two</b>')");
/// // What the script writes goes in right after it.
/// parser.insert("<b>two</b>");
/// assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
/// parser.finish(&mut doc);
/// let body = doc.body().unwrap();
/// assert_eq!(doc.text_content(body).unwrap(), "onedocument.write('<b>two</b>')twothree");
/// ```
pub struct DocumentParser {
    parser: Parser<Sink<'static>>,
    finished: bool,
}

impl std::fmt::Debug for DocumentParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentParser")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl DocumentParser {
    /// A parser that builds into `doc`, which should be a new, empty
    /// [`Document`]. `doc` is only borrowed here to set it up; pass the same
    /// document to every later call.
    pub fn new(doc: &mut Document, options: ParseOptions) -> Self {
        doc.set_scripting_enabled(options.scripting_enabled);
        let root = doc.root();
        let opts = ParseOpts {
            tree_builder: TreeBuilderOpts {
                scripting_enabled: options.scripting_enabled,
                ..TreeBuilderOpts::default()
            },
            ..ParseOpts::default()
        };
        let sink = Sink::with_slot(DocSlot::Lent(None), root, false);
        Self {
            parser: parse_document(sink, opts),
            finished: false,
        }
    }

    /// A parser for a `text/plain` document: the input is shown verbatim in
    /// a `<pre>` element, as browsers display plain text and JSON.
    pub fn new_plain_text(doc: &mut Document) -> Self {
        let mut parser = Self::new(doc, ParseOptions::default());
        // https://html.spec.whatwg.org/#read-text: as if the tokenizer had
        // seen a <pre> start tag and a newline, then switched to PLAINTEXT.
        parser.push("<pre>\n");
        let step = parser.resume(doc);
        debug_assert_eq!(
            step,
            ParseStep::NeedInput,
            "a <pre> start tag is not a script"
        );
        parser.parser.tokenizer.set_plaintext_state();
        parser
    }

    /// Appends `text` to the end of the input.
    pub fn push(&mut self, text: &str) {
        if !text.is_empty() {
            self.parser.input_buffer.push_back(StrTendril::from(text));
        }
    }

    /// Inserts `text` at the insertion point: just after where parsing
    /// stopped, ahead of the input not parsed yet. This is what
    /// `document.write()` does from a script the parser ran; call it after
    /// the script returns, with everything it wrote.
    pub fn insert(&mut self, text: &str) {
        if !text.is_empty() {
            self.parser.input_buffer.push_front(StrTendril::from(text));
        }
    }

    /// Starts or stops logging the changes parsing makes to the tree, to be
    /// read with [`take_changes`](Self::take_changes). Stopping drops the
    /// changes not taken yet. Logging is off to begin with, and costs
    /// nothing then.
    pub fn log_changes(&mut self, on: bool) {
        let mut log = self.parser.tokenizer.sink.sink.log.borrow_mut();
        if on != log.is_some() {
            *log = on.then(ChangeLog::default);
        }
    }

    /// The changes logged since logging started or they were last taken,
    /// in the order they were made.
    pub fn take_changes(&mut self) -> Vec<TreeChange> {
        self.parser
            .tokenizer
            .sink
            .sink
            .log
            .borrow_mut()
            .as_mut()
            .map(ChangeLog::take)
            .unwrap_or_default()
    }

    /// Whether [`finish`](Self::finish) was called.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Parses as much of the input as possible, stopping after the next
    /// script element or when the input runs out.
    pub fn resume(&mut self, doc: &mut Document) -> ParseStep {
        if self.finished {
            return ParseStep::NeedInput;
        }
        self.with_doc(doc, |parser| {
            loop {
                match parser.tokenizer.feed(&parser.input_buffer) {
                    TokenizerResult::Done => return ParseStep::NeedInput,
                    TokenizerResult::Script(script) => return ParseStep::Script(script),
                    // The encoding was settled before decoding started, and
                    // the document is not re-parsed to change it.
                    TokenizerResult::EncodingIndicator(_) => {}
                }
            }
        })
    }

    /// Ends the input: parses what is left, without pausing for scripts
    /// (a script cut off by the end of the input never runs), and closes
    /// every open element. Further calls do nothing.
    pub fn finish(&mut self, doc: &mut Document) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.with_doc(doc, |parser| {
            // `feed` stops at each script; scripts reached now are skipped.
            while !matches!(
                parser.tokenizer.feed(&parser.input_buffer),
                TokenizerResult::Done
            ) {}
            parser.tokenizer.end();
            parser.tokenizer.sink.sink.end();
        });
        doc.shrink_to_fit();
    }

    /// Lends `doc` to the sink for the duration of `f`.
    fn with_doc<R>(&self, doc: &mut Document, f: impl FnOnce(&Parser<Sink<'static>>) -> R) -> R {
        /// Puts the document back when dropped, even if parsing panics, so
        /// the caller is not left holding an empty one.
        struct Restore<'a> {
            slot: &'a RefCell<DocSlot<'static>>,
            doc: &'a mut Document,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let DocSlot::Lent(lent) = &mut *self.slot.borrow_mut() {
                    if let Some(lent) = lent.take() {
                        *self.doc = lent;
                    }
                }
            }
        }

        let slot = &self.parser.tokenizer.sink.sink.doc;
        *slot.borrow_mut() = DocSlot::Lent(Some(std::mem::take(doc)));
        let _restore = Restore { slot, doc };
        f(&self.parser)
    }
}
