//! Incremental parsing with `DocumentParser`.

use std::collections::HashMap;

use dasher_dom::{Document, DocumentParser, Name, NodeId, ParseOptions, ParseStep, TreeChange};

const PAGE: &str = r#"<!DOCTYPE html>
<html lang=en><head><meta charset=utf-8><title>Stream &amp; test</title>
<script>var a = 1 < 2 && "</div>";</script>
<style>p > b { color: red }</style>
</head><body class=main>
<table><tr><td>cell<div>foster</div></td></tr>x</table>
<p>one<b>bold<i>both</b>italic</i>
<!-- comment -->
<noscript><p>no script</p></noscript>
<template><script>in template</script><li>item</template>
<svg viewBox="0 0 1 1"><circle r=1 /><foreignObject><p>html</p></foreignObject></svg>
<textarea>  &lt;raw&gt; </textarea>
<script src=a.js></script><pre>
line</pre>&eacute;&#x263a;&notit;
</body></html>"#;

/// Feeds `chunks` one at a time, collecting the scripts it pauses at.
fn stream(chunks: &[&str]) -> (Document, Vec<NodeId>) {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    let mut scripts = Vec::new();
    for chunk in chunks {
        parser.push(chunk);
        while let ParseStep::Script(s) = parser.resume(&mut doc) {
            scripts.push(s);
        }
    }
    parser.finish(&mut doc);
    assert!(parser.is_finished());
    (doc, scripts)
}

fn html(doc: &Document) -> String {
    doc.outer_html(doc.document_element().unwrap())
}

#[test]
fn any_chunking_builds_the_same_tree_as_parse() {
    let expected = html(&Document::parse(PAGE));
    let (doc, scripts) = stream(&[PAGE]);
    assert_eq!(html(&doc), expected);
    // The script in the template pauses too, but it is not in the document.
    assert_eq!(scripts.len(), 3);
    let connected: Vec<bool> = scripts.iter().map(|&s| doc.is_connected(s)).collect();
    assert_eq!(connected, [true, false, true]);

    // One character at a time, splitting tags, entities and `</script>`.
    let chars: Vec<String> = PAGE.chars().map(String::from).collect();
    let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
    let (doc, scripts) = stream(&refs);
    assert_eq!(html(&doc), expected);
    assert_eq!(scripts.len(), 3);
}

#[test]
fn pauses_after_each_script_with_its_text_in_place() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push(
        "<head><script>first()</script><body><p id=a>a</p><script src=x.js></script><p id=b>b",
    );

    let ParseStep::Script(first) = parser.resume(&mut doc) else {
        panic!("expected a script")
    };
    assert!(doc.is_html(first, Name::SCRIPT));
    assert_eq!(doc.text_content(first).as_deref(), Some("first()"));
    assert!(doc.is_connected(first));
    // Nothing after the script is parsed yet.
    assert!(doc.body().is_none());

    let ParseStep::Script(second) = parser.resume(&mut doc) else {
        panic!("expected a script")
    };
    assert_eq!(doc.attribute(second, "src"), Some("x.js"));
    assert!(doc.get_element_by_id(doc.root(), "a").is_some());
    assert!(doc.get_element_by_id(doc.root(), "b").is_none());

    assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
    // The tokenizer waits for more text before emitting the open paragraph.
    parser.finish(&mut doc);
    let b = doc.get_element_by_id(doc.root(), "b").unwrap();
    assert_eq!(doc.text_content(b).as_deref(), Some("b"));
    // Calls after finishing do nothing.
    parser.push("<p>late");
    assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
    parser.finish(&mut doc);
    assert_eq!(doc.elements_by_tag_name(doc.root(), "p").count(), 2);
}

#[test]
fn written_markup_goes_in_at_the_insertion_point() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<body>[<script>A</script>]");
    let ParseStep::Script(_) = parser.resume(&mut doc) else {
        panic!("expected A")
    };
    // A writes text and another script; the rest of the page waits.
    parser.insert("(a1<script>B</script>a2)");
    let ParseStep::Script(b) = parser.resume(&mut doc) else {
        panic!("expected B")
    };
    assert_eq!(doc.text_content(b).as_deref(), Some("B"));
    // B's output goes before what is left of A's.
    parser.insert("(b)");
    assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
    parser.finish(&mut doc);
    let body = doc.body().unwrap();
    assert_eq!(doc.text_content(body).as_deref(), Some("[A(a1B(b)a2)]"));
}

#[test]
fn a_write_can_finish_a_tag_the_script_left_open() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<body><script>w</script> id=x>text</div>");
    assert!(matches!(parser.resume(&mut doc), ParseStep::Script(_)));
    parser.insert("<div");
    parser.finish(&mut doc);
    let x = doc.get_element_by_id(doc.root(), "x").unwrap();
    assert!(doc.is_html(x, Name::DIV));
    assert_eq!(doc.text_content(x).as_deref(), Some("text"));
}

#[test]
fn keeps_building_after_script_detaches_the_open_element() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<body><div id=box><script>remove</script><p>inside</p></div><p>after</p>");
    let ParseStep::Script(_) = parser.resume(&mut doc) else {
        panic!("expected a script")
    };
    // What `document.getElementById('box').remove()` would do.
    let boxed = doc.get_element_by_id(doc.root(), "box").unwrap();
    doc.remove(boxed);
    parser.finish(&mut doc);
    // The rest of the box went into the detached element; the page goes on.
    assert_eq!(doc.text_content(boxed).as_deref(), Some("removeinside"));
    assert_eq!(
        doc.text_content(doc.body().unwrap()).as_deref(),
        Some("after")
    );
}

#[test]
fn scripts_reached_only_at_the_end_never_pause() {
    let (doc, scripts) = stream(&["<script>one</script><script>two</script><script>cut off"]);
    // All three are in the tree, but `finish` does not stop for them.
    assert_eq!(scripts.len(), 2);
    assert_eq!(doc.elements_by_tag_name(doc.root(), "script").count(), 3);

    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<script>never paused</script><p>x");
    parser.finish(&mut doc);
    assert_eq!(doc.elements_by_tag_name(doc.root(), "p").count(), 1);
}

#[test]
fn noscript_is_raw_text_when_scripting_is_enabled() {
    let (doc, scripts) = stream(&["<noscript><script>no</script><p>fallback</p></noscript>"]);
    assert!(scripts.is_empty());
    assert_eq!(doc.elements_by_tag_name(doc.root(), "p").count(), 0);

    let mut doc = Document::new();
    let mut parser = DocumentParser::new(
        &mut doc,
        ParseOptions {
            scripting_enabled: false,
        },
    );
    parser.push("<noscript><p>fallback</p></noscript>");
    parser.finish(&mut doc);
    assert!(!doc.scripting_enabled());
    assert_eq!(doc.elements_by_tag_name(doc.root(), "p").count(), 1);
}

#[test]
fn plain_text_is_shown_verbatim_in_a_pre() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new_plain_text(&mut doc);
    parser.push("{\"html\": \"<b>not bold</b>\",\n");
    assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
    parser.push("\"script\": \"<script>x</script>\"}");
    assert_eq!(parser.resume(&mut doc), ParseStep::NeedInput);
    parser.finish(&mut doc);
    let body = doc.body().unwrap();
    assert_eq!(doc.children(body).count(), 1);
    let pre = doc.first_child(body).unwrap();
    assert!(doc.is_html(pre, Name::PRE));
    assert_eq!(
        doc.text_content(pre).as_deref(),
        Some("{\"html\": \"<b>not bold</b>\",\n\"script\": \"<script>x</script>\"}")
    );
}

/// Children per parent, as rebuilt from nothing but the logged changes.
#[derive(Default)]
struct Replay(HashMap<NodeId, Vec<NodeId>>);

impl Replay {
    fn apply(&mut self, change: TreeChange) {
        match change {
            TreeChange::Inserted {
                parent,
                node,
                prev,
                next,
            } => {
                let kids = self.0.entry(parent).or_default();
                let at = prev.map_or(0, |p| {
                    kids.iter().position(|&k| k == p).expect("prev is a child") + 1
                });
                assert_eq!(kids.get(at).copied(), next, "next sibling of {node:?}");
                kids.insert(at, node);
            }
            TreeChange::Removed {
                parent,
                node,
                prev,
                next,
            } => {
                let kids = self.0.get_mut(&parent).expect("parent has children");
                let at = kids.iter().position(|&k| k == node).expect("is a child");
                assert_eq!(at.checked_sub(1).map(|i| kids[i]), prev);
                assert_eq!(kids.get(at + 1).copied(), next);
                kids.remove(at);
            }
            TreeChange::TextAppended { .. } | TreeChange::AttributeAdded { .. } => {}
        }
    }

    /// Checks the replayed tree under `node` against the real one.
    fn check(&self, doc: &Document, node: NodeId) {
        let real: Vec<NodeId> = doc.children(node).collect();
        let replayed = self.0.get(&node).cloned().unwrap_or_default();
        assert_eq!(replayed, real, "children of {node:?}");
        for child in real {
            self.check(doc, child);
            if let Some(contents) = doc.template_contents(child) {
                self.check(doc, contents);
            }
        }
    }
}

/// Streams `chunks` with change logging on, replaying the changes taken
/// after each step.
fn stream_logged(chunks: &[&str]) -> (Document, Replay) {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.log_changes(true);
    let mut replay = Replay::default();
    for chunk in chunks {
        parser.push(chunk);
        loop {
            let step = parser.resume(&mut doc);
            parser
                .take_changes()
                .into_iter()
                .for_each(|c| replay.apply(c));
            if step == ParseStep::NeedInput {
                break;
            }
        }
    }
    parser.finish(&mut doc);
    parser
        .take_changes()
        .into_iter()
        .for_each(|c| replay.apply(c));
    (doc, replay)
}

#[test]
fn logged_changes_rebuild_the_tree() {
    // PAGE has foster parenting and misnested formatting elements, which
    // move nodes after inserting them.
    let (doc, replay) = stream_logged(&[PAGE]);
    replay.check(&doc, doc.root());

    let chars: Vec<String> = PAGE.chars().map(String::from).collect();
    let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
    let (doc, replay) = stream_logged(&refs);
    replay.check(&doc, doc.root());
    assert_eq!(html(&doc), html(&Document::parse(PAGE)));
}

#[test]
fn text_added_to_an_earlier_text_node_is_logged_once() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.log_changes(true);
    parser.push("<p>ab");
    parser.resume(&mut doc);
    let changes = parser.take_changes();
    // html, head, body, p and the text, which grew while it was new.
    assert_eq!(changes.len(), 5);
    assert!(
        changes
            .iter()
            .all(|c| matches!(c, TreeChange::Inserted { .. }))
    );
    let p = doc.first_child(doc.body().unwrap()).unwrap();
    let text = doc.first_child(p).unwrap();

    // Character references split the text into several appends.
    parser.push("c&amp;d");
    parser.resume(&mut doc);
    assert_eq!(
        parser.take_changes(),
        [TreeChange::TextAppended {
            node: text,
            old_len: 2
        }]
    );
    parser.push("e<p>");
    parser.resume(&mut doc);
    let changes = parser.take_changes();
    assert_eq!(
        changes[0],
        TreeChange::TextAppended {
            node: text,
            old_len: 5
        }
    );
    assert_eq!(doc.text(text), Some("abc&de"));

    // Nothing is logged once logging stops.
    parser.log_changes(false);
    parser.push("more");
    parser.finish(&mut doc);
    assert!(parser.take_changes().is_empty());
}

#[test]
fn attributes_from_a_repeated_body_tag_are_logged() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<body class=a>");
    parser.resume(&mut doc);
    parser.log_changes(true);
    parser.push("<body class=b id=x>");
    parser.resume(&mut doc);
    let body = doc.body().unwrap();
    let id = doc.intern("id");
    assert_eq!(
        parser.take_changes(),
        [TreeChange::AttributeAdded {
            node: body,
            namespace: Name::EMPTY,
            local: id,
        }]
    );
    assert_eq!(doc.attribute(body, "class"), Some("a"));
}

#[test]
fn logging_is_off_by_default() {
    let mut doc = Document::new();
    let mut parser = DocumentParser::new(&mut doc, ParseOptions::default());
    parser.push("<p>text");
    parser.finish(&mut doc);
    assert!(parser.take_changes().is_empty());
}
