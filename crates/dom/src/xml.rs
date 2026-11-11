//! A small, non-validating XML parser, for the XML documents scripts make
//! with `DOMParser` and `XMLHttpRequest.responseXML`.
//!
//! It handles what real-world XML (feeds, sitemaps, SVG, API responses)
//! uses: elements and attributes with namespaces, text, CDATA sections,
//! comments, character references and the predefined entities, and a
//! doctype, whose internal subset is skipped. Processing instructions and
//! the XML declaration are dropped, since the arena has no node for them,
//! and entities a DTD declares are not expanded. Anything not well-formed
//! is an error, as in browsers, which then show a `<parsererror>` document.

use std::fmt;

use crate::document::Document;
use crate::name::Name;
use crate::node::{Attr, NodeId, NodeKind};

/// Why a string is not a well-formed XML document, and where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct XmlError {
    pub message: String,
    /// 1-based line of the error.
    pub line: usize,
    /// 1-based column of the error, in characters.
    pub column: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for XmlError {}

impl Document {
    /// Parses an XML document into `document`, an empty document node made
    /// by [`create_document`](Self::create_document).
    ///
    /// On an error nothing is left under `document`. Parsing stops early,
    /// keeping what was built, once the [node limit](Self::set_node_limit)
    /// is exceeded.
    ///
    /// # Panics
    ///
    /// If `document` is not an empty document node.
    pub fn parse_xml_into(&mut self, document: NodeId, xml: &str) -> Result<()> {
        assert!(
            self.kind(document) == NodeKind::Document && self.first_child(document).is_none(),
            "can only parse into an empty document node"
        );
        let mut parser = Parser {
            doc: self,
            src: xml.strip_prefix('\u{feff}').unwrap_or(xml),
            pos: 0,
            document,
            open: Vec::new(),
            bindings: Vec::new(),
            text: String::new(),
            has_root: false,
        };
        let result = parser.run();
        if result.is_err() {
            let built: Vec<NodeId> = self.children(document).collect();
            for node in built {
                self.detach(node);
                self.release(node);
            }
        }
        result
    }
}

/// An open element: its node, its qualified name in the source and how many
/// namespace bindings were in scope outside it.
struct Open<'a> {
    node: NodeId,
    name: &'a str,
    bindings: usize,
}

struct Parser<'d, 'a> {
    doc: &'d mut Document,
    src: &'a str,
    pos: usize,
    document: NodeId,
    open: Vec<Open<'a>>,
    /// Namespace prefixes in scope, innermost last; `""` is the default.
    bindings: Vec<(&'a str, Name)>,
    /// Character data not yet turned into a text node.
    text: String,
    has_root: bool,
}

type Result<T> = std::result::Result<T, XmlError>;

/// An attribute as written in a start tag.
struct RawAttr<'a> {
    name: &'a str,
    value: String,
}

fn is_name_char(c: char) -> bool {
    !(c.is_ascii_whitespace() || matches!(c, '/' | '>' | '<' | '=' | '"' | '\'' | '&' | '?' | '!'))
}

fn is_name_start(c: char) -> bool {
    is_name_char(c) && !c.is_ascii_digit() && !matches!(c, '-' | '.')
}

fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

impl<'a> Parser<'_, 'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T> {
        let before = &self.src[..self.pos.min(self.src.len())];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Err(XmlError {
            message: message.into(),
            line,
            column: before[line_start..].chars().count() + 1,
        })
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> Result<()> {
        if self.eat(s) {
            Ok(())
        } else {
            self.error(format!("expected '{s}'"))
        }
    }

    fn skip_space(&mut self) -> bool {
        let rest = self.rest();
        let trimmed = rest.trim_start_matches(is_xml_space);
        self.pos += rest.len() - trimmed.len();
        trimmed.len() != rest.len()
    }

    /// Everything up to `end`, which is consumed too.
    fn until(&mut self, end: &str, what: &str) -> Result<&'a str> {
        let rest = self.rest();
        match rest.find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(&rest[..i])
            }
            None => {
                self.pos = self.src.len();
                self.error(format!("unterminated {what}"))
            }
        }
    }

    fn name(&mut self) -> Result<&'a str> {
        let rest = self.rest();
        if !rest.starts_with(is_name_start) {
            return self.error("expected a name");
        }
        let end = rest.find(|c| !is_name_char(c)).unwrap_or(rest.len());
        self.pos += end;
        Ok(&rest[..end])
    }

    fn parent(&self) -> NodeId {
        self.open.last().map_or(self.document, |o| o.node)
    }

    fn append(&mut self, node: NodeId) {
        let parent = self.parent();
        self.doc.insert_unchecked(parent, node, None);
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let t = self.doc.create_text(&self.text);
            self.text.clear();
            self.append(t);
        }
    }

    fn run(&mut self) -> Result<()> {
        while self.pos < self.src.len() {
            if self.doc.node_limit_exceeded() {
                return Ok(());
            }
            if self.eat("<!--") {
                let comment = self.until("-->", "comment")?;
                self.flush_text();
                let c = self.doc.create_comment(comment);
                self.append(c);
            } else if self.rest().starts_with("<![CDATA[") {
                if self.open.is_empty() {
                    return self.error("CDATA section outside the root element");
                }
                self.pos += "<![CDATA[".len();
                let data = self.until("]]>", "CDATA section")?;
                push_normalized(&mut self.text, data);
            } else if self.rest().starts_with("<!DOCTYPE") {
                if self.has_root {
                    return self.error("doctype after the root element");
                }
                self.pos += "<!DOCTYPE".len();
                self.doctype()?;
            } else if self.eat("<?") {
                // The XML declaration and processing instructions.
                self.until("?>", "processing instruction")?;
            } else if self.eat("</") {
                self.end_tag()?;
            } else if self.rest().starts_with('<') {
                self.pos += 1;
                self.start_tag()?;
            } else {
                self.char_data()?;
            }
        }
        if let Some(open) = self.open.last() {
            let name = open.name;
            return self.error(format!("unclosed element <{name}>"));
        }
        if !self.has_root {
            return self.error("no root element");
        }
        Ok(())
    }

    fn char_data(&mut self) -> Result<()> {
        let rest = self.rest();
        let end = rest.find('<').unwrap_or(rest.len());
        let raw = &rest[..end];
        if self.open.is_empty() {
            if !raw.chars().all(is_xml_space) {
                return self.error("text outside the root element");
            }
            self.pos += end;
            return Ok(());
        }
        if raw.contains("]]>") {
            return self.error("']]>' in text");
        }
        let mut text = std::mem::take(&mut self.text);
        let result = self.decode(raw, &mut text, false);
        self.text = text;
        result?;
        self.pos += end;
        Ok(())
    }

    /// Appends `raw` to `out` with references expanded and line breaks
    /// normalized; attribute values also turn whitespace into spaces.
    fn decode(&self, raw: &str, out: &mut String, attribute: bool) -> Result<()> {
        let mut rest = raw;
        while let Some(i) = rest.find(['&', '\r', '\n', '\t']) {
            out.push_str(&rest[..i]);
            let c = rest.as_bytes()[i];
            rest = &rest[i + 1..];
            match c {
                b'\r' => {
                    rest = rest.strip_prefix('\n').unwrap_or(rest);
                    out.push(if attribute { ' ' } else { '\n' });
                }
                b'\n' | b'\t' => out.push(if attribute { ' ' } else { c as char }),
                _ => {
                    let Some(end) = rest.find(';') else {
                        return self.error("unterminated reference");
                    };
                    let name = &rest[..end];
                    rest = &rest[end + 1..];
                    let c = match name {
                        "lt" => '<',
                        "gt" => '>',
                        "amp" => '&',
                        "apos" => '\'',
                        "quot" => '"',
                        _ => match name.strip_prefix('#') {
                            Some(num) => {
                                let code = match num.strip_prefix('x') {
                                    Some(hex) => u32::from_str_radix(hex, 16),
                                    None => num.parse(),
                                };
                                match code.ok().and_then(char::from_u32) {
                                    Some(c) if c != '\0' => c,
                                    _ => {
                                        return self.error(format!(
                                            "invalid character reference '&{name};'"
                                        ));
                                    }
                                }
                            }
                            None => return self.error(format!("undefined entity '&{name};'")),
                        },
                    };
                    out.push(c);
                }
            }
        }
        out.push_str(rest);
        Ok(())
    }

    fn quoted(&mut self) -> Result<&'a str> {
        let quote = match self.rest().chars().next() {
            Some(q @ ('"' | '\'')) => q,
            _ => return self.error("expected a quoted value"),
        };
        self.pos += 1;
        let rest = self.rest();
        match rest.find(quote) {
            Some(i) => {
                self.pos += i + 1;
                Ok(&rest[..i])
            }
            None => self.error("unterminated quoted value"),
        }
    }

    fn doctype(&mut self) -> Result<()> {
        if !self.skip_space() {
            return self.error("expected whitespace after '<!DOCTYPE'");
        }
        let name = self.name()?;
        self.skip_space();
        let (mut public_id, mut system_id) = ("", "");
        if self.eat("PUBLIC") {
            self.skip_space();
            public_id = self.quoted()?;
            self.skip_space();
            system_id = self.quoted()?;
        } else if self.eat("SYSTEM") {
            self.skip_space();
            system_id = self.quoted()?;
        }
        self.skip_space();
        if self.eat("[") {
            // The internal subset: skipped, minding quotes and comments that
            // may hold a ']'.
            loop {
                let rest = self.rest();
                let Some(i) = rest.find([']', '"', '\'', '<']) else {
                    self.pos = self.src.len();
                    return self.error("unterminated doctype");
                };
                self.pos += i;
                if self.eat("]") {
                    break;
                } else if self.eat("<!--") {
                    self.until("-->", "comment")?;
                } else if self.rest().starts_with('<') {
                    self.pos += 1;
                } else {
                    self.quoted()?;
                }
            }
            self.skip_space();
        }
        self.expect(">")?;
        let dt = self.doc.create_doctype(name, public_id, system_id);
        self.append(dt);
        Ok(())
    }

    fn end_tag(&mut self) -> Result<()> {
        let name = self.name()?;
        self.skip_space();
        self.expect(">")?;
        match self.open.last() {
            Some(open) if open.name == name => {}
            Some(open) => {
                let expected = open.name;
                return self.error(format!(
                    "end tag </{name}> does not match the open element <{expected}>"
                ));
            }
            None => return self.error(format!("end tag </{name}> with no open element")),
        }
        self.flush_text();
        let open = self.open.pop().expect("checked above");
        self.bindings.truncate(open.bindings);
        Ok(())
    }

    fn start_tag(&mut self) -> Result<()> {
        if self.open.is_empty() && self.has_root {
            return self.error("a second root element");
        }
        let name = self.name()?;
        let mut attrs: Vec<RawAttr<'a>> = Vec::new();
        let empty = loop {
            let spaced = self.skip_space();
            if self.eat("/>") {
                break true;
            }
            if self.eat(">") {
                break false;
            }
            if !spaced {
                return self.error("expected whitespace between attributes");
            }
            let attr = self.name()?;
            self.skip_space();
            self.expect("=")?;
            self.skip_space();
            let raw = self.quoted()?;
            if raw.contains('<') {
                return self.error("'<' in an attribute value");
            }
            if attrs.iter().any(|a| a.name == attr) {
                return self.error(format!("duplicate attribute '{attr}'"));
            }
            let mut value = String::new();
            self.decode(raw, &mut value, true)?;
            attrs.push(RawAttr { name: attr, value });
        };

        let outer = self.bindings.len();
        for a in &attrs {
            let prefix = if a.name == "xmlns" {
                ""
            } else if let Some(p) = a.name.strip_prefix("xmlns:") {
                if a.value.is_empty() {
                    return self.error(format!("prefix '{p}' bound to no namespace"));
                }
                p
            } else {
                continue;
            };
            let ns = self.doc.intern(&a.value);
            self.bindings.push((prefix, ns));
        }
        let (ns, prefix, local) = self.resolve(name, true)?;
        let mut records: Vec<Attr> = Vec::with_capacity(attrs.len());
        for a in &attrs {
            let (ans, aprefix, alocal) = if a.name == "xmlns" {
                (Name::NS_XMLNS, Name::EMPTY, Name::XMLNS)
            } else if let Some(p) = a.name.strip_prefix("xmlns:") {
                (Name::NS_XMLNS, Name::XMLNS, self.doc.intern(p))
            } else {
                self.resolve(a.name, false)?
            };
            let attr = self.doc.new_attr(ans, aprefix, alocal, &a.value);
            if records
                .iter()
                .any(|r| r.local == alocal && self.doc.attr_namespace(r) == ans)
            {
                return self.error(format!("duplicate attribute '{}'", a.name));
            }
            records.push(attr);
        }

        self.flush_text();
        let el = self
            .doc
            .create_element_with_attrs(ns, local, records.into_boxed_slice());
        self.doc.set_element_prefix(el, prefix);
        self.append(el);
        self.has_root = true;
        if empty {
            self.bindings.truncate(outer);
        } else {
            self.open.push(Open {
                node: el,
                name,
                bindings: outer,
            });
        }
        Ok(())
    }

    /// The namespace, prefix and local name of a qualified name. Unprefixed
    /// element names take the default namespace; attribute names none.
    fn resolve(&mut self, qname: &str, element: bool) -> Result<(Name, Name, Name)> {
        let Some((prefix, local)) = qname.split_once(':') else {
            let ns = if element {
                self.lookup("").unwrap_or(Name::EMPTY)
            } else {
                Name::EMPTY
            };
            return Ok((ns, Name::EMPTY, self.doc.intern(qname)));
        };
        if prefix.is_empty() || local.is_empty() || local.contains(':') {
            return self.error(format!("'{qname}' is not a valid qualified name"));
        }
        let ns = if prefix == "xml" {
            Name::NS_XML
        } else {
            match self.lookup(prefix) {
                Some(ns) => ns,
                None => return self.error(format!("unbound namespace prefix '{prefix}'")),
            }
        };
        Ok((ns, self.doc.intern(prefix), self.doc.intern(local)))
    }

    fn lookup(&self, prefix: &str) -> Option<Name> {
        self.bindings
            .iter()
            .rev()
            .find(|(p, _)| *p == prefix)
            .map(|&(_, ns)| ns)
            .filter(|&ns| ns != Name::EMPTY)
    }
}

/// Appends CDATA content with line breaks normalized.
fn push_normalized(out: &mut String, data: &str) {
    if !data.contains('\r') {
        out.push_str(data);
        return;
    }
    out.push_str(&data.replace("\r\n", "\n").replace('\r', "\n"));
}
