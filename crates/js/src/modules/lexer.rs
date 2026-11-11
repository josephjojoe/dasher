//! Finds the imports in JS source without parsing it.
//!
//! The module loader needs two things from a script before QuickJS sees
//! it: the specifiers of its static imports (`import … from "x"`,
//! `export … from "x"`), so the whole module graph can be fetched in
//! parallel up front, and where its dynamic `import(…)` calls are, so they
//! can be pointed at the loader's own asynchronous import (QuickJS loads
//! modules synchronously and cannot wait for the network).
//!
//! This is a tokenizer, not a parser, in the manner of es-module-lexer: it
//! skips comments, strings, template literals and regular expressions, and
//! looks at the `import` and `export` keywords in between. Whether a `/`
//! starts a regular expression or is a division is decided from the token
//! before it, which is right for all but contrived code. A mistake costs
//! little: a missed import is loaded on demand when QuickJS asks for it,
//! and a stray one is an extra fetch whose failure nothing depends on.

/// What [`scan`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// The specifiers of static imports and re-exports, in source order,
    /// with escapes decoded.
    pub(crate) imports: Vec<String>,
    /// Byte offsets of the `import` keywords of dynamic `import(…)` calls.
    pub(crate) dynamic: Vec<usize>,
}

/// Scans `source` for static imports and dynamic import calls.
pub(crate) fn scan(source: &str) -> Scan {
    let mut lexer = Lexer::new(source.as_bytes());
    lexer.run();
    Scan {
        imports: lexer.imports,
        dynamic: lexer.dynamic,
    }
}

/// Whether `source` might hold an import at all; a quick check that lets
/// most classic scripts skip [`scan`].
pub(crate) fn may_import(source: &str) -> bool {
    source.contains("import")
}

/// Keywords after which a `/` starts a regular expression.
const REGEX_KEYWORDS: &[&[u8]] = &[
    b"return",
    b"typeof",
    b"instanceof",
    b"in",
    b"of",
    b"new",
    b"delete",
    b"void",
    b"throw",
    b"case",
    b"do",
    b"else",
    b"yield",
    b"await",
];

/// Keywords whose parenthesized part may be followed by a statement, so a
/// `/` after the closing parenthesis starts a regular expression.
const PAREN_KEYWORDS: &[&[u8]] = &[b"if", b"while", b"for", b"with"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paren {
    Plain,
    /// After `if`, `while`, `for` or `with`.
    Keyword,
    /// The argument list of the dynamic import at this offset.
    Import(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Brace {
    Plain,
    /// `${` in a template literal.
    Template,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Whether a `/` here would start a regular expression.
    regex_ok: bool,
    /// Whether the last token was `.` or `?.`, making an identifier here a
    /// property name.
    after_dot: bool,
    /// What a `(` here would open.
    next_paren: Paren,
    parens: Vec<Paren>,
    braces: Vec<Brace>,
    /// A dynamic import whose `)` was just seen: it is a call unless a `{`
    /// follows, which makes it a method named `import`.
    closed_import: Option<usize>,
    imports: Vec<String>,
    dynamic: Vec<usize>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self {
            src,
            pos: 0,
            regex_ok: true,
            after_dot: false,
            next_paren: Paren::Plain,
            parens: Vec::new(),
            braces: Vec::new(),
            closed_import: None,
            imports: Vec::new(),
            dynamic: Vec::new(),
        }
    }

    fn peek(&self, offset: usize) -> u8 {
        self.src.get(self.pos + offset).copied().unwrap_or(0)
    }

    fn run(&mut self) {
        if self.src.starts_with(b"#!") {
            self.pos = line_end(self.src, 0);
        }
        loop {
            self.pos = skip_trivia(self.src, self.pos);
            let Some(&c) = self.src.get(self.pos) else {
                break;
            };
            if let Some(offset) = self.closed_import.take() {
                if c != b'{' {
                    self.dynamic.push(offset);
                }
            }
            let next_paren = std::mem::replace(&mut self.next_paren, Paren::Plain);
            let after_dot = std::mem::replace(&mut self.after_dot, false);
            match c {
                b'"' | b'\'' => {
                    self.pos = string_end(self.src, self.pos).unwrap_or(self.src.len());
                    self.regex_ok = false;
                }
                b'`' => {
                    self.pos += 1;
                    self.template();
                }
                b'/' => {
                    if self.regex_ok {
                        self.pos = regex_end(self.src, self.pos);
                        self.regex_ok = false;
                    } else {
                        self.pos += 1;
                        self.regex_ok = true;
                    }
                }
                b'{' => {
                    self.braces.push(Brace::Plain);
                    self.pos += 1;
                    self.regex_ok = true;
                }
                b'}' => {
                    self.pos += 1;
                    if self.braces.pop() == Some(Brace::Template) {
                        self.template();
                    } else {
                        self.regex_ok = true;
                    }
                }
                b'(' => {
                    self.parens.push(next_paren);
                    self.pos += 1;
                    self.regex_ok = true;
                }
                b')' => {
                    self.pos += 1;
                    let paren = self.parens.pop().unwrap_or(Paren::Plain);
                    self.regex_ok = paren == Paren::Keyword;
                    if let Paren::Import(offset) = paren {
                        self.closed_import = Some(offset);
                    }
                }
                b']' => {
                    self.pos += 1;
                    self.regex_ok = false;
                }
                b'.' if self.peek(1).is_ascii_digit() => self.number(),
                b'.' => {
                    if self.src[self.pos..].starts_with(b"...") {
                        self.pos += 3;
                        self.regex_ok = true;
                    } else {
                        self.pos += 1;
                        self.after_dot = true;
                        self.regex_ok = false;
                    }
                }
                b'?' if self.peek(1) == b'.' && !self.peek(2).is_ascii_digit() => {
                    self.pos += 2;
                    self.after_dot = true;
                    self.regex_ok = false;
                }
                b'+' | b'-' if self.peek(1) == c => {
                    self.pos += 2;
                    self.regex_ok = false;
                }
                c if c.is_ascii_digit() => self.number(),
                c if is_ident_start(c) => self.word(after_dot),
                _ => {
                    self.pos += 1;
                    self.regex_ok = true;
                }
            }
        }
        if let Some(offset) = self.closed_import {
            self.dynamic.push(offset);
        }
    }

    /// Scans template literal text from just after a backtick or the `}`
    /// closing a substitution, up to the closing backtick or the next `${`.
    fn template(&mut self) {
        while let Some(&c) = self.src.get(self.pos) {
            match c {
                b'\\' => self.pos += 2,
                b'`' => {
                    self.pos += 1;
                    self.regex_ok = false;
                    return;
                }
                b'$' if self.peek(1) == b'{' => {
                    self.pos += 2;
                    self.braces.push(Brace::Template);
                    self.regex_ok = true;
                    return;
                }
                _ => self.pos += 1,
            }
        }
        self.pos = self.src.len();
    }

    fn number(&mut self) {
        let start = self.pos;
        let hex = self.src[start..].len() > 1
            && self.src[start] == b'0'
            && matches!(self.src[start + 1], b'x' | b'X');
        while let Some(&c) = self.src.get(self.pos) {
            let exponent_sign =
                matches!(c, b'+' | b'-') && !hex && matches!(self.src[self.pos - 1], b'e' | b'E');
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || exponent_sign {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.regex_ok = false;
    }

    /// An identifier, keyword or private name.
    fn word(&mut self, after_dot: bool) {
        let start = self.pos;
        self.pos = ident_end(self.src, start);
        let word = &self.src[start..self.pos];
        if after_dot {
            self.regex_ok = false;
            return;
        }
        match word {
            b"import" => self.import(start),
            b"export" => self.export(),
            w if PAREN_KEYWORDS.contains(&w) => {
                self.next_paren = Paren::Keyword;
                self.regex_ok = true;
            }
            w => self.regex_ok = REGEX_KEYWORDS.contains(&w),
        }
    }

    /// Just after the `import` keyword at `start`.
    fn import(&mut self, start: usize) {
        let next = skip_trivia(self.src, self.pos);
        match self.src.get(next) {
            Some(b'(') => {
                self.next_paren = Paren::Import(start);
                self.regex_ok = true;
            }
            // import.meta
            Some(b'.') => self.regex_ok = false,
            _ => {
                if let Some((specifier, end)) = import_declaration(self.src, self.pos) {
                    self.imports.push(specifier);
                    self.pos = end;
                }
                self.regex_ok = true;
            }
        }
    }

    /// Just after the `export` keyword.
    fn export(&mut self) {
        if let Some((specifier, end)) = re_export(self.src, self.pos) {
            self.imports.push(specifier);
            self.pos = end;
        }
        self.regex_ok = true;
    }
}

/// Reads the rest of an import declaration after `import`: either a
/// specifier straight away (`import "x"`) or bindings followed by
/// `from "x"`. Returns the specifier and where it ends.
fn import_declaration(src: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut after_from = false;
    loop {
        pos = skip_trivia(src, pos);
        match *src.get(pos)? {
            b'"' | b'\'' => {
                // `import "x"` or `import x from "x"`; a string anywhere
                // else is an arbitrary module export name inside braces,
                // which `skip_braces` steps over.
                return string_at(src, pos);
            }
            b'{' if !after_from => pos = skip_braces(src, pos)?,
            b'*' | b',' if !after_from => pos += 1,
            c if is_ident_start(c) && !after_from => {
                let end = ident_end(src, pos);
                after_from = &src[pos..end] == b"from" && next_is_string(src, end);
                pos = end;
            }
            _ => return None,
        }
    }
}

/// Reads the rest of a re-export after `export`: `* from "x"`,
/// `* as name from "x"` or `{ … } from "x"`. Returns the specifier and where
/// it ends, or `None` for any other export.
fn re_export(src: &[u8], pos: usize) -> Option<(String, usize)> {
    let mut pos = skip_trivia(src, pos);
    match *src.get(pos)? {
        b'*' => {
            pos = skip_trivia(src, pos + 1);
            if src[pos..].starts_with(b"as") && !is_ident_part(*src.get(pos + 2)?) {
                pos = skip_trivia(src, pos + 2);
                pos = match *src.get(pos)? {
                    b'"' | b'\'' => string_end(src, pos)?,
                    c if is_ident_start(c) => ident_end(src, pos),
                    _ => return None,
                };
            }
        }
        b'{' => pos = skip_braces(src, pos)?,
        _ => return None,
    }
    pos = skip_trivia(src, pos);
    let end = ident_end(src, pos);
    if &src[pos..end] != b"from" {
        return None;
    }
    string_at(src, skip_trivia(src, end))
}

fn next_is_string(src: &[u8], pos: usize) -> bool {
    matches!(src.get(skip_trivia(src, pos)), Some(b'"' | b'\''))
}

/// Skips a `{ … }` of import or export bindings, which hold only names,
/// commas and strings. Returns where it ends.
fn skip_braces(src: &[u8], mut pos: usize) -> Option<usize> {
    pos += 1;
    loop {
        pos = skip_trivia(src, pos);
        match *src.get(pos)? {
            b'}' => return Some(pos + 1),
            b'"' | b'\'' => pos = string_end(src, pos)?,
            b',' => pos += 1,
            c if is_ident_start(c) => pos = ident_end(src, pos),
            _ => return None,
        }
    }
}

/// The string literal at `pos`, decoded, and where it ends.
fn string_at(src: &[u8], pos: usize) -> Option<(String, usize)> {
    if !matches!(src.get(pos), Some(b'"' | b'\'')) {
        return None;
    }
    let end = string_end(src, pos)?;
    let raw = std::str::from_utf8(&src[pos + 1..end - 1]).ok()?;
    Some((decode_string(raw)?, end))
}

/// Where the string literal starting at `pos` ends, or `None` if it does
/// not end on its line.
fn string_end(src: &[u8], pos: usize) -> Option<usize> {
    let quote = src[pos];
    let mut i = pos + 1;
    while let Some(&c) = src.get(i) {
        match c {
            b'\\' => i += 2,
            b'\n' | b'\r' => return None,
            c if c == quote => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Where the regular expression literal starting at `pos` ends. A literal
/// that runs into a line break was not one: lexing goes on after the `/`.
fn regex_end(src: &[u8], pos: usize) -> usize {
    let mut i = pos + 1;
    let mut in_class = false;
    while let Some(&c) = src.get(i) {
        match c {
            b'\\' => i += 2,
            b'\n' | b'\r' => return pos + 1,
            b'[' => {
                in_class = true;
                i += 1;
            }
            b']' => {
                in_class = false;
                i += 1;
            }
            b'/' if !in_class => return ident_end(src, i + 1),
            _ => i += 1,
        }
    }
    pos + 1
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || matches!(c, b'_' | b'$' | b'\\' | b'#') || c >= 0x80
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

fn ident_end(src: &[u8], mut pos: usize) -> usize {
    while let Some(&c) = src.get(pos) {
        if !is_ident_part(c) {
            break;
        }
        // An escape such as `i` is part of the identifier.
        pos += if c == b'\\' { 2 } else { 1 };
    }
    pos.min(src.len())
}

fn line_end(src: &[u8], pos: usize) -> usize {
    src[pos..]
        .iter()
        .position(|&c| c == b'\n' || c == b'\r')
        .map_or(src.len(), |i| pos + i)
}

/// Skips whitespace and comments.
fn skip_trivia(src: &[u8], mut pos: usize) -> usize {
    while let Some(&c) = src.get(pos) {
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => pos += 1,
            b'/' if src.get(pos + 1) == Some(&b'/') => pos = line_end(src, pos),
            b'/' if src.get(pos + 1) == Some(&b'*') => {
                pos = src[pos + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(src.len(), |i| pos + 2 + i + 2);
            }
            // No-break space, BOM, line and paragraph separators.
            0xc2 if src.get(pos + 1) == Some(&0xa0) => pos += 2,
            0xef if src[pos..].starts_with(b"\xef\xbb\xbf") => pos += 3,
            0xe2 if src[pos..].starts_with(b"\xe2\x80\xa8")
                || src[pos..].starts_with(b"\xe2\x80\xa9") =>
            {
                pos += 3
            }
            _ => break,
        }
    }
    pos
}

/// Decodes the escapes in the text of a string literal.
fn decode_string(raw: &str) -> Option<String> {
    if !raw.contains('\\') {
        return Some(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0' => out.push('\0'),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'u' => {
                let rest = chars.as_str();
                let (hex, len) = if let Some(braced) = rest.strip_prefix('{') {
                    let close = braced.find('}')?;
                    (&braced[..close], close + 2)
                } else {
                    (rest.get(..4)?, 4)
                };
                let code = u32::from_str_radix(hex, 16).ok()?;
                // Lone surrogates cannot be in a Rust string.
                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                chars = rest[len..].chars();
            }
            // A line continuation.
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => {}
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imports(source: &str) -> Vec<String> {
        scan(source).imports
    }

    /// The source with every dynamic import found marked as `IMPORT`.
    fn marked(source: &str) -> String {
        let mut out = source.to_owned();
        for &offset in scan(source).dynamic.iter().rev() {
            out.replace_range(offset..offset + 6, "IMPORT");
        }
        out
    }

    #[test]
    fn static_imports() {
        let source = r#"
            import "./side-effect.js";
            import def from './default.js';
            import * as ns from "./namespace.js";
            import { a, b as c, "string name" as d } from "./named.js";
            import def2, { e } from "./both.js";
            import def3, * as ns2 from "./both-ns.js"
            import {
                f,
                // a comment
                g, /* another */
            } from "./multiline.js";
            import from from "./from.js";
            import { from as x } from "./from-named.js";
            import json from "./data.json" with { type: "json" };
            import/**/"./tight.js";
        "#;
        assert_eq!(
            imports(source),
            [
                "./side-effect.js",
                "./default.js",
                "./namespace.js",
                "./named.js",
                "./both.js",
                "./both-ns.js",
                "./multiline.js",
                "./from.js",
                "./from-named.js",
                "./data.json",
                "./tight.js",
            ]
        );
    }

    #[test]
    fn minified_imports() {
        let source = r#"import{a as b,c}from"./a.js";import d from"./d.js";import*as e from"./e.js";export*from"./f.js";export{g}from"./g.js";const h=1;export{h};"#;
        assert_eq!(
            imports(source),
            ["./a.js", "./d.js", "./e.js", "./f.js", "./g.js"]
        );
    }

    #[test]
    fn re_exports() {
        let source = r#"
            export * from "./all.js";
            export * as ns from "./ns.js";
            export * as "string" from "./ns-string.js";
            export { a, b as c } from "./named.js";
            export { default } from "./default.js";
            export { x };
            export const y = "./not-an-import.js";
            export default function () {}
            export { z as from };
        "#;
        assert_eq!(
            imports(source),
            [
                "./all.js",
                "./ns.js",
                "./ns-string.js",
                "./named.js",
                "./default.js"
            ]
        );
    }

    #[test]
    fn decodes_specifier_escapes() {
        assert_eq!(
            imports(r#"import "./a\x62\u{63}.js"; import './it\'s.js';"#),
            ["./abc.js", "./it's.js"]
        );
    }

    #[test]
    fn ignores_imports_in_strings_comments_templates_and_regexes() {
        let source = r#"
            const s = "import x from './string.js'";
            const t = 'import("./single.js")';
            // import "./line-comment.js";
            /* import "./block-comment.js"; import("./block.js") */
            const u = `import "./template.js" ${ "import('./in-substitution.js')" } import('./after.js')`;
            const r = /import "\/regex.js"[/]import("x")/g;
            const q = a / b / import("./division.js");
        "#;
        assert!(imports(source).is_empty());
        let dynamic = scan(source).dynamic;
        assert_eq!(dynamic.len(), 1);
        assert!(source[dynamic[0]..].starts_with(r#"import("./division.js")"#));
    }

    #[test]
    fn dynamic_imports() {
        let source = r#"
            const a = import("./a.js");
            const b = await import ( "./b.js" , { with: { type: "json" } } );
            import(`./${name}.js`).then(m => m.default);
            const lazy = () => import("./c.js");
            if (x) import("./d.js")
            const meta = import.meta.url;
            foo.import("not-dynamic");
            foo?.import("not-dynamic");
            const o = { import(x) { return x; }, import: 1 };
            class K { static import(y) {} async import() {} }
        "#;
        let expected = r#"
            const a = IMPORT("./a.js");
            const b = await IMPORT ( "./b.js" , { with: { type: "json" } } );
            IMPORT(`./${name}.js`).then(m => m.default);
            const lazy = () => IMPORT("./c.js");
            if (x) IMPORT("./d.js")
            const meta = import.meta.url;
            foo.import("not-dynamic");
            foo?.import("not-dynamic");
            const o = { import(x) { return x; }, import: 1 };
            class K { static import(y) {} async import() {} }
        "#;
        assert_eq!(marked(source), expected);
        assert!(imports(source).is_empty());
    }

    #[test]
    fn nested_templates_and_braces() {
        let source = r#"
            const s = `a${ {x: `b${ `c${ import("./deep.js") }` }`}.x }d`;
            function f() { return `}` + import("./after.js"); }
        "#;
        assert_eq!(scan(source).dynamic.len(), 2);
    }

    #[test]
    fn regexes_after_keywords_and_statements() {
        let source = r#"
            if (a) /"/.test(b); import("./one.js");
            return /'/; import("./two.js");
            x = typeof /`/; import("./three.js");
            function f() {} /"/.exec(s); import("./four.js");
            const half = y / 2, quarter = (z) / 4; import("./five.js");
            const n = 1e-5 / 2, m = 0x1e-1 / 3; import("./six.js");
            i++ / 2; import("./seven.js");
            const cls = /[/"]/; import("./eight.js");
            a.return / 2 / import("./nine.js");
        "#;
        assert_eq!(scan(source).dynamic.len(), 9);
    }

    #[test]
    fn hashbang_and_unterminated_input() {
        assert_eq!(imports("#!/usr/bin/env node\nimport './a.js'"), ["./a.js"]);
        // Nothing to find, but nothing to panic about either.
        for source in [
            "import",
            "import {",
            "import x from",
            "export *",
            "export * as",
            "`unterminated ${",
            "'open",
            "/open",
            "import(",
            "/* open",
            "\\",
        ] {
            let _ = scan(source);
        }
    }

    #[test]
    fn quick_check() {
        assert!(may_import("import('x')"));
        assert!(!may_import("console.log(1)"));
    }
}
