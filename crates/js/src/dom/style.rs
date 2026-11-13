//! Inline style declarations and the synthetic computed style, as plain
//! Rust: parsing and serializing a `style` attribute, the property name
//! conversions `CSSStyleDeclaration` does, and the values
//! `getComputedStyle` reports. The objects scripts see are in the `host`
//! module; this is what they compute with.
//!
//! This is not a CSS parser. A `style` attribute is split into
//! declarations on the semicolons outside strings and brackets, each split
//! at its first colon, with a trailing `!important` taken off; values are
//! kept as written (trimmed). That is all pages need from inline styles
//! here, and it is what `dom.js` did before these moved to Rust.

use dasher_dom::{Document, Name, NodeId};

/// One declaration: a property name (lower case unless custom, `--x`), its
/// value and whether it is `!important`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decl {
    pub(crate) name: Box<str>,
    pub(crate) value: Box<str>,
    pub(crate) important: bool,
}

/// The declarations of a `style` attribute, parsed once and kept until the
/// attribute changes. Reads compare the attribute with the text the
/// declarations came from, so a change made any way at all (`setAttribute`,
/// `className`-style writes, the parser, cloning) is noticed on the next
/// read, and the common case costs one comparison.
#[derive(Default)]
pub(crate) struct Declarations {
    /// The attribute value `decls` were parsed from, or `None` for no
    /// attribute.
    text: Option<Box<str>>,
    decls: Vec<Decl>,
}

impl Declarations {
    /// The declarations for the attribute value `text`, parsed again only if
    /// it changed.
    pub(crate) fn get(&mut self, text: Option<&str>) -> &[Decl] {
        if self.text.as_deref() != text {
            self.decls = text.map(parse).unwrap_or_default();
            self.text = text.map(Into::into);
        }
        &self.decls
    }

    /// Records that the attribute was written as `text`, the serialization
    /// of `decls`, so the next read need not parse it.
    pub(crate) fn set(&mut self, text: &str, decls: Vec<Decl>) {
        self.text = Some(text.into());
        self.decls = decls;
    }
}

/// The value of the `style` attribute `getAttribute('style')` finds: the
/// first one with no prefix.
pub(crate) fn style_attribute(doc: &Document, id: NodeId) -> Option<&str> {
    doc.attrs(id)
        .iter()
        .find(|a| a.local() == Name::STYLE && doc.attr_prefix(a) == Name::EMPTY)
        .map(|a| a.value())
}

/// Whether JS's `String.prototype.trim` removes `c`.
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// Parses declarations: split on semicolons outside strings and brackets,
/// each at its first colon. A later declaration of a property replaces an
/// earlier one (and moves to the end); empty names and values are dropped.
pub(crate) fn parse(text: &str) -> Vec<Decl> {
    let mut decls: Vec<Decl> = Vec::new();
    let mut push = |part: &str| {
        let Some(colon) = part.find(':') else { return };
        if colon == 0 {
            return;
        }
        let name = js_trim(&part[..colon]);
        let (value, important) = strip_important(js_trim(&part[colon + 1..]));
        if name.is_empty() || value.is_empty() {
            return;
        }
        let name = lower_name(name);
        decls.retain(|d| *d.name != *name);
        decls.push(Decl {
            name: name.into(),
            value: value.into(),
            important,
        });
    };
    let bytes = text.as_bytes();
    let mut depth = 0u32;
    let mut quote = 0u8;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'(' || c == b'[' {
            depth += 1;
        } else if (c == b')' || c == b']') && depth > 0 {
            depth -= 1;
        } else if c == b';' && depth == 0 {
            push(&text[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    // A backslash at the very end skips past it; `start` is always on a
    // character boundary (just after an ASCII `;`).
    push(text.get(start..).unwrap_or(""));
    decls
}

/// Takes a trailing `!important` (any case, spaces allowed after the `!`)
/// off a trimmed value.
fn strip_important(value: &str) -> (&str, bool) {
    let rest = value.trim_end_matches(js_space);
    let n = "important".len();
    if rest.len() < n || !rest.is_char_boundary(rest.len() - n) {
        return (value, false);
    }
    let (head, word) = rest.split_at(rest.len() - n);
    if !word.eq_ignore_ascii_case("important") {
        return (value, false);
    }
    let head = head.trim_end_matches(js_space);
    match head.strip_suffix('!') {
        Some(head) => (js_trim(head), true),
        None => (value, false),
    }
}

/// A property name as declarations store it: lower case, unless it is a
/// custom property (`--x`), which is case-sensitive.
pub(crate) fn lower_name(name: &str) -> String {
    if name.starts_with("--") {
        name.to_owned()
    } else {
        name.to_lowercase()
    }
}

/// Writes declarations back as a `style` attribute value.
pub(crate) fn serialize(decls: &[Decl]) -> String {
    let mut out = String::new();
    for d in decls {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&d.name);
        out.push_str(": ");
        out.push_str(&d.value);
        if d.important {
            out.push_str(" !important");
        }
        out.push(';');
    }
    out
}

/// Whether `key` reads as a CSS property of a `CSSStyleDeclaration`
/// (`style.marginTop`, `style['margin-top']`, `style.webkitTransform`):
/// `/^-?-?[a-zA-Z][a-zA-Z0-9-]*$/`.
pub(crate) fn is_property_key(key: &str) -> bool {
    let b = key.as_bytes();
    let skip = b.iter().take(2).take_while(|&&c| c == b'-').count();
    match b.get(skip) {
        Some(c) if c.is_ascii_alphabetic() => b[skip + 1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'-'),
        _ => false,
    }
}

/// The property name for a key read off a `CSSStyleDeclaration`:
/// camelCase becomes dashed (`marginTop` -> `margin-top`, `webkitX` and
/// `WebkitX` -> `-webkit-x`, `cssFloat` -> `float`); dashed names are
/// lower-cased, custom properties kept as they are.
pub(crate) fn dashed(key: &str) -> String {
    if key.starts_with("--") || key.contains('-') {
        return if key.to_lowercase().starts_with("--") {
            key.to_owned()
        } else {
            key.to_lowercase()
        };
    }
    if key == "cssFloat" {
        return "float".to_owned();
    }
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if ["webkit-", "moz-", "ms-", "o-"]
        .iter()
        .any(|p| out.starts_with(p))
    {
        out.insert(0, '-');
    }
    out
}

// -------------------------------------------------------------------------
// The computed style.

/// The properties `getComputedStyle` lists, in order, before any others
/// the inline style sets. `None` values depend on the element (`display`,
/// `width`, `height`).
pub(crate) const COMPUTED: &[(&str, Option<&str>)] = &[
    ("display", None),
    ("visibility", Some("visible")),
    ("opacity", Some("1")),
    ("position", Some("static")),
    ("float", Some("none")),
    ("overflow", Some("visible")),
    ("overflow-x", Some("visible")),
    ("overflow-y", Some("visible")),
    ("width", None),
    ("height", None),
    ("top", Some("auto")),
    ("left", Some("auto")),
    ("right", Some("auto")),
    ("bottom", Some("auto")),
    ("z-index", Some("auto")),
    ("font-size", Some("16px")),
    ("font-family", Some("Arial, sans-serif")),
    ("font-weight", Some("400")),
    ("font-style", Some("normal")),
    ("line-height", Some("normal")),
    ("color", Some("rgb(0, 0, 0)")),
    ("background-color", Some("rgba(0, 0, 0, 0)")),
    ("background-image", Some("none")),
    ("box-sizing", Some("content-box")),
    ("transform", Some("none")),
    ("pointer-events", Some("auto")),
    ("cursor", Some("auto")),
    ("white-space", Some("normal")),
    ("text-align", Some("start")),
    ("text-decoration", Some("none")),
    ("direction", Some("ltr")),
    ("content", Some("normal")),
    ("clip", Some("auto")),
    ("clip-path", Some("none")),
    ("filter", Some("none")),
    ("transition", Some("all 0s ease 0s")),
    ("animation-name", Some("none")),
    ("animation-duration", Some("0s")),
    ("transition-duration", Some("0s")),
    ("scroll-behavior", Some("auto")),
    ("margin-top", Some("0px")),
    ("padding-top", Some("0px")),
    ("border-top-width", Some("0px")),
    ("margin-right", Some("0px")),
    ("padding-right", Some("0px")),
    ("border-right-width", Some("0px")),
    ("margin-bottom", Some("0px")),
    ("padding-bottom", Some("0px")),
    ("border-bottom-width", Some("0px")),
    ("margin-left", Some("0px")),
    ("padding-left", Some("0px")),
    ("border-left-width", Some("0px")),
];

/// The default `display` of an element: by tag for HTML elements,
/// `inline` for everything else.
pub(crate) fn default_display(doc: &Document, id: NodeId) -> &'static str {
    if doc.namespace(id) != Some(Name::NS_HTML) {
        return "inline";
    }
    let Some(local) = doc.local_name(id) else {
        return "inline";
    };
    match doc.name_str(local) {
        "address" | "article" | "aside" | "blockquote" | "body" | "center" | "details"
        | "dialog" | "dd" | "div" | "dl" | "dt" | "fieldset" | "figcaption" | "figure"
        | "footer" | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup"
        | "hr" | "html" | "legend" | "listing" | "main" | "menu" | "nav" | "ol" | "p"
        | "plaintext" | "pre" | "search" | "section" | "summary" | "ul" | "xmp" => "block",
        "li" => "list-item",
        "table" => "table",
        "tr" => "table-row",
        "td" | "th" => "table-cell",
        "tbody" => "table-row-group",
        "thead" => "table-header-group",
        "tfoot" => "table-footer-group",
        "caption" => "table-caption",
        "col" => "table-column",
        "colgroup" => "table-column-group",
        "button" | "input" | "select" | "textarea" | "img" | "video" | "canvas" | "iframe"
        | "meter" | "progress" => "inline-block",
        "area" | "base" | "datalist" | "head" | "link" | "meta" | "noscript" | "script"
        | "style" | "template" | "title" | "param" | "source" | "track" => "none",
        _ => "inline",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(decls: &[Decl]) -> Vec<(&str, &str, bool)> {
        decls
            .iter()
            .map(|d| (&*d.name, &*d.value, d.important))
            .collect()
    }

    #[test]
    fn parses_declarations() {
        let d = parse("color: red; MARGIN-TOP:4px !important;;--X: a;b; :c; d:");
        assert_eq!(
            names(&d),
            [
                ("color", "red", false),
                ("margin-top", "4px", true),
                ("--X", "a", false)
            ]
        );
        let d = parse("background: url('a;b') no-repeat; content: \"x;\\\"y\"; grid: [a;b] 1fr");
        assert_eq!(
            names(&d),
            [
                ("background", "url('a;b') no-repeat", false),
                ("content", "\"x;\\\"y\"", false),
                ("grid", "[a;b] 1fr", false)
            ]
        );
        // A repeated property moves to the end with its last value.
        let d = parse("a: 1; b: 2; a: 3 ! IMPORTANT ");
        assert_eq!(names(&d), [("b", "2", false), ("a", "3", true)]);
        assert_eq!(names(&parse("x: !important")), []);
        assert_eq!(
            names(&parse("x: é;y:日本")),
            [("x", "é", false), ("y", "日本", false)]
        );
        assert_eq!(names(&parse("x: 'a\\")), [("x", "'a\\", false)]);
    }

    #[test]
    fn serializes() {
        let d = parse("color:red;margin-top: 4px!important");
        assert_eq!(serialize(&d), "color: red; margin-top: 4px !important;");
        assert_eq!(serialize(&[]), "");
    }

    #[test]
    fn caches_until_the_text_changes() {
        let mut d = Declarations::default();
        assert!(d.get(None).is_empty());
        assert_eq!(d.get(Some("a: 1")).len(), 1);
        assert_eq!(d.get(Some("a: 1; b: 2")).len(), 2);
        assert!(d.get(None).is_empty());
        let decls = parse("c: 3");
        d.set("c: 3;", decls);
        assert_eq!(names(d.get(Some("c: 3;"))), [("c", "3", false)]);
    }

    #[test]
    fn property_keys() {
        for key in [
            "display",
            "marginTop",
            "-webkit-x",
            "--custom",
            "x1",
            "WebkitX",
        ] {
            assert!(is_property_key(key), "{key}");
        }
        for key in ["", "-", "---x", "1x", "a_b", "a b", "é", "--"] {
            assert!(!is_property_key(key), "{key}");
        }
        assert_eq!(dashed("marginTop"), "margin-top");
        assert_eq!(dashed("cssFloat"), "float");
        assert_eq!(dashed("WebkitTransform"), "-webkit-transform");
        assert_eq!(dashed("webkitTransform"), "-webkit-transform");
        assert_eq!(dashed("msFlex"), "-ms-flex");
        assert_eq!(dashed("Margin-Top"), "margin-top");
        assert_eq!(dashed("--Custom"), "--Custom");
        assert_eq!(dashed("color"), "color");
    }
}
