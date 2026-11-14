//! `Range`: boundary points, contents, and `createContextualFragment`.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions, WindowOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/".into(),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

const TREE: &str =
    "<!doctype html><div id=a><p id=p>one<b id=b>two</b>three</p><p id=q>four</p></div>";

/// Helpers the tests share: `$` by id, `html` of a fragment, `at` for a
/// range's boundary points, `err` for the name of what a call throws.
const PRELUDE: &str = r#"
    const $ = (id) => document.getElementById(id);
    const html = (fragment) => {
      const div = document.createElement('div');
      div.appendChild(fragment);
      return div.innerHTML;
    };
    const name = (n) => n.id || n.nodeName + (n.nodeType === 3 ? ':' + n.data : '');
    const at = (r) => [name(r.startContainer), r.startOffset, name(r.endContainer), r.endOffset];
    const err = (f) => { try { f(); return 'none'; } catch (e) { return e.name; } };
"#;

fn eval_with_prelude(js: &JsRuntime, source: &str) -> String {
    eval(js, &format!("{PRELUDE}{source}"))
}

#[test]
fn a_new_range_is_collapsed_at_the_document() {
    let js = page(TREE);
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        const made = new Range();
        [at(r), r.collapsed, r instanceof Range, String(r), Object.prototype.toString.call(r),
         at(made), r.commonAncestorContainer === document,
         [Range.START_TO_START, Range.START_TO_END, r.END_TO_END, r.END_TO_START]]
        "#,
    );
    assert_eq!(
        out,
        r##"[["#document",0,"#document",0],true,true,"","[object Range]",["#document",0,"#document",0],true,[0,1,2,3]]"##
    );
}

#[test]
fn boundary_points_move_and_stay_ordered() {
    let js = page(TREE);
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        const out = [];
        r.selectNodeContents($('p'));
        out.push(at(r), String(r));
        r.setStart($('p').firstChild, 1);
        r.setEnd($('q').firstChild, 2);
        out.push(at(r), String(r), r.commonAncestorContainer.id, r.collapsed);
        // An end before the start moves the start with it.
        r.setEnd($('p'), 0);
        out.push(at(r), r.collapsed);
        // A start after the end moves the end.
        r.setStart($('q'), 1);
        out.push(at(r));
        r.selectNode($('b'));
        out.push(at(r), String(r));
        r.setStartBefore($('p'));
        r.setEndAfter($('q'));
        out.push(at(r), String(r));
        r.setStartAfter($('p'));
        r.setEndBefore($('q'));
        out.push(at(r));
        r.selectNodeContents($('a'));
        r.collapse(true);
        out.push(at(r));
        r.selectNodeContents($('a'));
        r.collapse();
        out.push(at(r));
        out
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r##"[["p",0,"p",3],"onetwothree","##,
            r##"["#text:one",1,"#text:four",2],"netwothreefo","a",false,"##,
            r##"["p",0,"p",0],true,"##,
            r##"["q",1,"q",1],"##,
            r##"["p",1,"p",2],"two","##,
            r##"["a",0,"a",2],"onetwothreefour","##,
            r##"["a",1,"a",1],"##,
            r##"["a",0,"a",0],"##,
            r##"["a",2,"a",2]]"##
        )
    );
}

#[test]
fn comparisons_between_points_ranges_and_nodes() {
    let js = page(TREE);
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        r.setStart($('p').firstChild, 1);
        r.setEnd($('b').firstChild, 2);
        const other = document.createRange();
        other.selectNode($('b'));
        const detached = document.createElement('div');
        [
          [r.compareBoundaryPoints(Range.START_TO_START, other), r.compareBoundaryPoints(Range.START_TO_END, other),
           r.compareBoundaryPoints(Range.END_TO_END, other), r.compareBoundaryPoints(Range.END_TO_START, other),
           r.compareBoundaryPoints(Range.START_TO_START, r.cloneRange())],
          [r.isPointInRange($('p'), 1), r.isPointInRange($('p'), 0), r.isPointInRange($('q'), 0),
           r.isPointInRange(detached, 0)],
          [r.comparePoint($('p'), 0), r.comparePoint($('b'), 0), r.comparePoint($('p'), 2)],
          [r.intersectsNode($('b')), r.intersectsNode($('q')), r.intersectsNode($('p').lastChild),
           r.intersectsNode($('a')), r.intersectsNode(detached)],
          [err(() => r.setStart(document.doctype, 0)), err(() => r.setStart($('p'), 4)),
           err(() => r.selectNode(detached)), err(() => r.comparePoint(detached, 0)),
           err(() => r.compareBoundaryPoints(4, other)), err(() => r.setStart()),
           err(() => r.setStart({}, 0))],
        ]
        "#,
    );
    assert_eq!(
        out,
        concat!(
            "[[-1,1,-1,-1,0],[true,false,false,false],[-1,0,1],[true,false,false,true,false],",
            r#"["InvalidNodeTypeError","IndexSizeError","InvalidNodeTypeError","WrongDocumentError","#,
            r#""NotSupportedError","TypeError","TypeError"]]"#
        )
    );
}

#[test]
fn a_clone_of_a_range_moves_on_its_own() {
    let js = page(TREE);
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        r.selectNode($('b'));
        const copy = r.cloneRange();
        copy.selectNodeContents($('q'));
        r.detach();
        [at(r), at(copy)]
        "#,
    );
    assert_eq!(out, r#"[["p",1,"p",2],["q",0,"q",1]]"#);
}

#[test]
fn contents_are_cloned_extracted_and_deleted() {
    let js = page(TREE);
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        r.setStart($('p').firstChild, 1);
        r.setEnd($('q').firstChild, 2);
        const out = [];
        out.push(html(r.cloneContents()), $('a').innerHTML, at(r));
        out.push(html(r.extractContents()), $('a').innerHTML, at(r));
        // Within one text node.
        r.setStart($('q').firstChild, 0);
        r.setEnd($('q').firstChild, 1);
        out.push(html(r.cloneContents()), html(r.extractContents()), $('q').innerHTML, at(r));
        // Whole children.
        r.selectNodeContents($('a'));
        r.deleteContents();
        out.push($('a').innerHTML, at(r), html(r.extractContents()));
        out
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"["<p id=\"p\">ne<b id=\"b\">two</b>three</p><p id=\"q\">fo</p>","#,
            r#""<p id=\"p\">one<b id=\"b\">two</b>three</p><p id=\"q\">four</p>","#,
            r##"["#text:one",1,"#text:four",2],"##,
            r#""<p id=\"p\">ne<b id=\"b\">two</b>three</p><p id=\"q\">fo</p>","#,
            r#""<p id=\"p\">o</p><p id=\"q\">ur</p>",["a",1,"a",1],"#,
            r##""u","u","r",["#text:r",0,"#text:r",0],"##,
            r#""",["a",0,"a",0],""]"#
        )
    );
}

#[test]
fn nodes_are_inserted_and_contents_surrounded() {
    let js = page("<!doctype html><p id=p>hello</p><p id=q>a<b id=b>bc</b>d</p>");
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        const out = [];
        // Into a text node, which is split.
        r.setStart($('p').firstChild, 2);
        r.insertNode(document.createElement('i'));
        out.push($('p').innerHTML, at(r));
        // Into an element, a fragment of two nodes.
        const two = document.createDocumentFragment();
        two.append(document.createElement('u'), 'x');
        r.setStart($('p'), 0);
        r.collapse(true);
        r.insertNode(two);
        out.push($('p').innerHTML, at(r));
        // Around part of a text node. Extracting leaves an empty text node
        // before the new parent, as in browsers.
        r.setStart($('q').firstChild, 0);
        r.setEnd($('q').firstChild, 1);
        const span = document.createElement('span');
        span.textContent = 'gone';
        r.surroundContents(span);
        out.push($('q').innerHTML, at(r), String(r));
        // Not around half of an element.
        r.setStart($('q'), 0);
        r.setEnd($('b').firstChild, 1);
        out.push(err(() => r.surroundContents(document.createElement('em'))),
                 err(() => r.insertNode($('q'))), $('q').innerHTML);
        r.selectNodeContents($('b'));
        out.push(err(() => r.surroundContents(document.createDocumentFragment())));
        out
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r##"["he<i></i>llo",["#text:he",2,"p",2],"##,
            r##""<u></u>xhe<i></i>llo",["p",0,"p",2],"##,
            r##""<span>a</span><b id=\"b\">bc</b>d",["q",1,"q",2],"a","##,
            r##""InvalidStateError","HierarchyRequestError","<span>a</span><b id=\"b\">bc</b>d","##,
            r##""InvalidNodeTypeError"]"##
        )
    );
}

#[test]
fn a_contextual_fragment_parses_in_the_context_of_the_start_node() {
    let js = page(
        "<!doctype html><table><tbody id=body><tr id=row><td>1</td></tr></tbody></table><p id=p>text</p>",
    );
    let out = eval_with_prelude(
        &js,
        r#"
        const r = document.createRange();
        const out = [];
        // The document, and the root element, parse as <body> does: no rows.
        const loose = r.createContextualFragment('<tr><td>x</td></tr><p>y</p>');
        out.push(loose.nodeType, loose instanceof DocumentFragment, html(loose));
        r.selectNode(document.body);
        out.push(html(r.createContextualFragment('<td>x</td>')));
        // A table section takes rows, a row takes cells.
        r.selectNodeContents($('body'));
        out.push(html(r.createContextualFragment('<tr><td>2</td></tr>')));
        r.selectNodeContents($('row'));
        out.push(html(r.createContextualFragment('<td>3</td>')));
        // A text node: its parent element.
        r.setStart($('row').firstChild.firstChild, 0);
        out.push(html(r.createContextualFragment('<td>4</td>b')));
        r.setStart($('p').firstChild, 2);
        out.push(html(r.createContextualFragment('<b>5</b>')), $('p').innerHTML);
        out.push(err(() => r.createContextualFragment()));
        out
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[11,true,"x<p>y</p>","x","<tr><td>2</td></tr>","<td>3</td>","4b","<b>5</b>","text","#,
            r#""TypeError"]"#
        )
    );
}

#[test]
fn scripts_in_a_contextual_fragment_run_when_inserted() {
    let js = page("<!doctype html><div id=d></div>");
    let out = eval_with_prelude(
        &js,
        r#"
        globalThis.ran = [];
        const out = [];
        const fragment = document.createRange().createContextualFragment(
          '<p>x</p><script>ran.push("fragment:" + document.currentScript.parentNode.id)<\/script>' +
          '<div><script>ran.push("nested")<\/script></div><script type="text/plain">ran.push("data")<\/script>');
        out.push(ran.slice());
        $('d').appendChild(fragment);
        out.push(ran.slice());
        // Once run, a script does not run again when moved.
        document.body.appendChild($('d').querySelector('script'));
        out.push(ran.slice());
        // innerHTML's scripts never run.
        $('d').innerHTML = '<script>ran.push("innerHTML")<\/script>';
        out.push(ran.slice());
        // insertNode runs them too.
        const r = document.createRange();
        r.selectNodeContents($('d'));
        r.insertNode(r.createContextualFragment('<script>ran.push("insertNode")<\/script>'));
        out.push(ran.slice());
        out
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[[],["fragment:d","nested"],["fragment:d","nested"],["fragment:d","nested"],"#,
            r#"["fragment:d","nested","insertNode"]]"#
        )
    );
}
