//! Interned names for tags, attributes, namespaces and prefixes.
//!
//! Every name in a [`Document`](crate::Document) is a [`Name`]: a 4-byte
//! index into that document's [`Names`] table. Comparing two names is an
//! integer comparison, and each distinct string is stored once per document.
//!
//! A fixed set of well-known names (common HTML tags and attributes, the
//! namespaces and their prefixes) has the same id in every document and is
//! available as constants such as [`Name::DIV`] or [`Name::NS_HTML`], so code
//! can test for them without a lookup. Any other name is added to the
//! document's table the first time it is seen and is freed with the document.
//! Names are only meaningful within the document that interned them.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// An interned name, valid within the [`Names`] table that produced it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(u32);

impl Name {
    /// The raw id. Well-known names have the same id in every document.
    pub fn to_raw(self) -> u32 {
        self.0
    }

    /// Whether this is one of the well-known names shared by all documents.
    pub fn is_static(self) -> bool {
        (self.0 as usize) < STATIC_NAMES.len()
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match STATIC_NAMES.get(self.0 as usize) {
            Some(s) => write!(f, "Name({s:?})"),
            None => write!(f, "Name(#{})", self.0),
        }
    }
}

macro_rules! static_names {
    ($($konst:ident => $s:literal,)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u32)]
        enum Static { $($konst,)* }

        impl Name {
            $(
                #[doc = concat!("`", $s, "`")]
                pub const $konst: Name = Name(Static::$konst as u32);
            )*
        }

        const STATIC_NAMES: &[&str] = &[$($s,)*];

        fn static_name(s: &str) -> Option<Name> {
            match s {
                $($s => Some(Name::$konst),)*
                _ => None,
            }
        }
    };
}

static_names! {
    // The empty string: "no namespace" and "no prefix".
    EMPTY => "",

    // Namespaces.
    NS_HTML => "http://www.w3.org/1999/xhtml",
    NS_SVG => "http://www.w3.org/2000/svg",
    NS_MATHML => "http://www.w3.org/1998/Math/MathML",
    NS_XLINK => "http://www.w3.org/1999/xlink",
    NS_XML => "http://www.w3.org/XML/1998/namespace",
    NS_XMLNS => "http://www.w3.org/2000/xmlns/",

    // Prefixes of the namespaced attributes the HTML parser creates.
    XLINK => "xlink",
    XML => "xml",
    XMLNS => "xmlns",

    // Document metadata and scripting.
    HTML => "html",
    HEAD => "head",
    BODY => "body",
    TITLE => "title",
    BASE => "base",
    LINK => "link",
    META => "meta",
    STYLE => "style",
    SCRIPT => "script",
    NOSCRIPT => "noscript",
    TEMPLATE => "template",
    SLOT => "slot",

    // Sections and grouping.
    DIV => "div",
    SPAN => "span",
    P => "p",
    MAIN => "main",
    HEADER => "header",
    FOOTER => "footer",
    NAV => "nav",
    SECTION => "section",
    ARTICLE => "article",
    ASIDE => "aside",
    ADDRESS => "address",
    SEARCH => "search",
    H1 => "h1",
    H2 => "h2",
    H3 => "h3",
    H4 => "h4",
    H5 => "h5",
    H6 => "h6",
    HGROUP => "hgroup",
    HR => "hr",
    PRE => "pre",
    LISTING => "listing",
    BLOCKQUOTE => "blockquote",
    OL => "ol",
    UL => "ul",
    LI => "li",
    MENU => "menu",
    DL => "dl",
    DT => "dt",
    DD => "dd",
    FIGURE => "figure",
    FIGCAPTION => "figcaption",
    DETAILS => "details",
    SUMMARY => "summary",
    DIALOG => "dialog",
    CENTER => "center",

    // Text-level semantics.
    A => "a",
    EM => "em",
    STRONG => "strong",
    SMALL => "small",
    S => "s",
    CITE => "cite",
    Q => "q",
    DFN => "dfn",
    ABBR => "abbr",
    TIME => "time",
    CODE => "code",
    VAR => "var",
    SAMP => "samp",
    KBD => "kbd",
    SUB => "sub",
    SUP => "sup",
    I => "i",
    B => "b",
    U => "u",
    MARK => "mark",
    BDI => "bdi",
    BDO => "bdo",
    BR => "br",
    WBR => "wbr",
    INS => "ins",
    DEL => "del",
    FONT => "font",
    NOBR => "nobr",
    TT => "tt",
    BIG => "big",
    STRIKE => "strike",

    // Embedded content.
    IMG => "img",
    PICTURE => "picture",
    SOURCE => "source",
    IFRAME => "iframe",
    EMBED => "embed",
    OBJECT => "object",
    PARAM => "param",
    VIDEO => "video",
    AUDIO => "audio",
    TRACK => "track",
    MAP => "map",
    AREA => "area",
    CANVAS => "canvas",
    SVG => "svg",
    MATH => "math",
    PATH => "path",
    G => "g",

    // Tables.
    TABLE => "table",
    CAPTION => "caption",
    COLGROUP => "colgroup",
    COL => "col",
    THEAD => "thead",
    TBODY => "tbody",
    TFOOT => "tfoot",
    TR => "tr",
    TD => "td",
    TH => "th",

    // Forms.
    FORM => "form",
    LABEL => "label",
    INPUT => "input",
    BUTTON => "button",
    SELECT => "select",
    DATALIST => "datalist",
    OPTGROUP => "optgroup",
    OPTION => "option",
    TEXTAREA => "textarea",
    OUTPUT => "output",
    PROGRESS => "progress",
    METER => "meter",
    FIELDSET => "fieldset",
    LEGEND => "legend",

    // Obsolete elements that still affect parsing or serialization.
    XMP => "xmp",
    NOEMBED => "noembed",
    NOFRAMES => "noframes",
    PLAINTEXT => "plaintext",
    FRAME => "frame",
    FRAMESET => "frameset",
    BASEFONT => "basefont",
    BGSOUND => "bgsound",
    KEYGEN => "keygen",

    // Attributes.
    ID => "id",
    CLASS => "class",
    HREF => "href",
    SRC => "src",
    SRCSET => "srcset",
    SIZES => "sizes",
    ALT => "alt",
    TYPE => "type",
    NAME => "name",
    VALUE => "value",
    REL => "rel",
    CONTENT => "content",
    CHARSET => "charset",
    HTTP_EQUIV => "http-equiv",
    PROPERTY => "property",
    ITEMPROP => "itemprop",
    LANG => "lang",
    DIR => "dir",
    WIDTH => "width",
    HEIGHT => "height",
    ACTION => "action",
    METHOD => "method",
    TARGET => "target",
    PLACEHOLDER => "placeholder",
    DISABLED => "disabled",
    CHECKED => "checked",
    SELECTED => "selected",
    READONLY => "readonly",
    REQUIRED => "required",
    MULTIPLE => "multiple",
    HIDDEN => "hidden",
    OPEN => "open",
    SIZE => "size",
    CONTENTEDITABLE => "contenteditable",
    TABINDEX => "tabindex",
    ROLE => "role",
    LOADING => "loading",
    ASYNC => "async",
    DEFER => "defer",
    NOMODULE => "nomodule",
    CROSSORIGIN => "crossorigin",
    INTEGRITY => "integrity",
    NONCE => "nonce",
    REFERRERPOLICY => "referrerpolicy",
    FOR => "for",
    COLSPAN => "colspan",
    ROWSPAN => "rowspan",
    MEDIA => "media",
    DOWNLOAD => "download",
    ENCODING => "encoding",
    VIEWBOX => "viewBox",
    D => "d",
    FILL => "fill",
    ARIA_LABEL => "aria-label",
    ARIA_HIDDEN => "aria-hidden",
}

/// A document's table of interned names.
///
/// Well-known names never touch the table; other names are stored once each
/// and live as long as the table.
#[derive(Default, Clone)]
pub struct Names {
    dynamic: Vec<Arc<str>>,
    lookup: HashMap<Arc<str>, Name>,
}

impl Names {
    /// An empty table, knowing only the well-known names.
    pub fn new() -> Self {
        Self::default()
    }

    /// The name for `s`, adding it to the table if it is new.
    pub fn intern(&mut self, s: &str) -> Name {
        if let Some(name) = self.get(s) {
            return name;
        }
        let id = STATIC_NAMES.len() + self.dynamic.len();
        let name = Name(u32::try_from(id).expect("more than u32::MAX distinct names"));
        let s: Arc<str> = Arc::from(s);
        self.dynamic.push(s.clone());
        self.lookup.insert(s, name);
        name
    }

    /// The name for `s` if it has been interned, without adding it.
    ///
    /// Useful for matching: if a string was never interned, no node in the
    /// document can carry it as a name.
    pub fn get(&self, s: &str) -> Option<Name> {
        static_name(s).or_else(|| self.lookup.get(s).copied())
    }

    /// The string a name stands for.
    ///
    /// # Panics
    ///
    /// If `name` did not come from this table.
    pub fn resolve(&self, name: Name) -> &str {
        let i = name.0 as usize;
        match STATIC_NAMES.get(i) {
            Some(s) => s,
            None => &self.dynamic[i - STATIC_NAMES.len()],
        }
    }

    /// The number of names added beyond the well-known ones.
    pub fn dynamic_len(&self) -> usize {
        self.dynamic.len()
    }
}

impl fmt::Debug for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Names")
            .field("dynamic", &self.dynamic)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_names_resolve_without_interning() {
        let names = Names::new();
        assert_eq!(names.get("div"), Some(Name::DIV));
        assert_eq!(names.resolve(Name::DIV), "div");
        assert_eq!(names.resolve(Name::EMPTY), "");
        assert_eq!(names.resolve(Name::NS_HTML), "http://www.w3.org/1999/xhtml");
        assert_eq!(names.resolve(Name::VIEWBOX), "viewBox");
        assert!(Name::ARIA_HIDDEN.is_static());
        assert_eq!(names.dynamic_len(), 0);
    }

    #[test]
    fn interning_is_idempotent_and_case_sensitive() {
        let mut names = Names::new();
        assert_eq!(names.intern("div"), Name::DIV);
        let a = names.intern("my-widget");
        let b = names.intern("my-widget");
        let c = names.intern("My-Widget");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(!a.is_static());
        assert_eq!(names.resolve(a), "my-widget");
        assert_eq!(names.resolve(c), "My-Widget");
        assert_eq!(names.dynamic_len(), 2);
    }

    #[test]
    fn get_does_not_intern() {
        let mut names = Names::new();
        assert_eq!(names.get("data-x"), None);
        let x = names.intern("data-x");
        assert_eq!(names.get("data-x"), Some(x));
    }

    #[test]
    fn static_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in STATIC_NAMES {
            assert!(seen.insert(s), "duplicate static name {s:?}");
        }
    }

    #[test]
    fn debug_shows_static_strings() {
        assert_eq!(format!("{:?}", Name::SPAN), "Name(\"span\")");
    }
}
