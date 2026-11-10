//! Selector parsing (Selectors Level 4), over the tokens of [`tokenizer`].
//!
//! The parser works on token slices: a functional pseudo-class's arguments
//! or an attribute selector's contents are found first, then parsed as a
//! slice of their own. Like the CSS parser, it closes blocks and strings left
//! open at the end of the input, so `a[href` is the same as `a[href]`.
//!
//! [`tokenizer`]: crate::tokenizer

use dasher_dom::Names;

use crate::error::SelectorError;
use crate::selector::{
    AttrCase, AttrOp, AttrSelector, Combinator, Complex, LocalName, NameRef, Nth, Part, Pseudo,
    Reach, Relative, SelectorList, Simple,
};
use crate::tokenizer::{Token, tokenize};

/// Why a selector is invalid. Becomes [`SelectorError::reason`].
type Reason = &'static str;

/// Compounds left to right, each with the combinator on its left.
type Compounds = Vec<(Box<[Simple]>, Combinator)>;

/// Parses a selector list, as `querySelectorAll` does.
pub(crate) fn parse(input: &str) -> Result<SelectorList, SelectorError> {
    let tokens = tokenize(input);
    let mut parser = Parser {
        names: Names::new(),
        nesting: 0,
    };
    parser
        .selector_list(&tokens, false)
        .map_err(|reason| SelectorError::new(input, reason))
}

/// HTML attributes whose values selectors match ASCII case-insensitively on
/// HTML elements (HTML §4.16.2, "case-sensitivity of selectors").
const CASE_INSENSITIVE_ATTRS: &[&str] = &[
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];

/// Pseudo-elements that parse (and never match), as `::name`.
const PSEUDO_ELEMENTS: &[&str] = &[
    "after",
    "backdrop",
    "before",
    "checkmark",
    "column",
    "cue",
    "details-content",
    "file-selector-button",
    "first-letter",
    "first-line",
    "grammar-error",
    "marker",
    "picker-icon",
    "placeholder",
    "scroll-marker",
    "scroll-marker-group",
    "search-text",
    "selection",
    "spelling-error",
    "target-text",
    "view-transition",
];

/// Functional pseudo-elements that parse (and never match), as `::name()`.
const PSEUDO_ELEMENT_FUNCTIONS: &[&str] = &[
    "cue",
    "highlight",
    "part",
    "picker",
    "slotted",
    "view-transition-group",
    "view-transition-image-pair",
    "view-transition-new",
    "view-transition-old",
];

/// Pseudo-classes for state dasher has no notion of (user interaction,
/// focus, media playback, history, form validation...). They parse, so
/// that selectors using them are not errors, but never match.
const NEVER_PSEUDO_CLASSES: &[&str] = &[
    "-webkit-autofill",
    "active",
    "autofill",
    "buffering",
    "current",
    "focus",
    "focus-visible",
    "focus-within",
    "fullscreen",
    "future",
    "host",
    "hover",
    "in-range",
    "invalid",
    "modal",
    "muted",
    "out-of-range",
    "past",
    "paused",
    "picture-in-picture",
    "playing",
    "popover-open",
    "seeking",
    "stalled",
    "target",
    "target-within",
    "user-invalid",
    "user-valid",
    "valid",
    "visited",
    "volume-locked",
];

/// Functional pseudo-classes that parse, whatever their arguments, and
/// never match.
const NEVER_PSEUDO_CLASS_FUNCTIONS: &[&str] = &["host", "host-context", "state"];

struct Parser {
    /// An empty table: only used to recognise well-known names.
    names: Names,
    /// How many logical pseudo-classes (`:is()`, `:not()`, ...) deep we are.
    /// Pseudo-elements are not allowed inside them.
    nesting: u32,
}

impl Parser {
    fn name_ref(&self, s: &str) -> NameRef {
        match self.names.get(s) {
            Some(name) => NameRef::Static(name),
            None => NameRef::Other(s.into()),
        }
    }

    fn local_name(&self, s: &str) -> LocalName {
        LocalName {
            lower: self.name_ref(&s.to_ascii_lowercase()),
            exact: self.name_ref(s),
        }
    }

    /// A comma-separated list of complex selectors. A forgiving list (as in
    /// `:is()` and `:where()`) drops the selectors that fail to parse
    /// instead of failing as a whole.
    fn selector_list(&mut self, t: &[Token], forgiving: bool) -> Result<SelectorList, Reason> {
        let mut selectors = Vec::new();
        for item in split_commas(t) {
            match self.complex(item) {
                Ok(complex) => selectors.push(complex),
                Err(reason) if !forgiving => return Err(reason),
                Err(_) => {}
            }
        }
        Ok(SelectorList {
            selectors: selectors.into(),
        })
    }

    /// A list of relative selectors, as in `:has()`.
    fn relative_list(&mut self, t: &[Token]) -> Result<Box<[Relative]>, Reason> {
        split_commas(t)
            .into_iter()
            .map(|item| self.relative(item))
            .collect()
    }

    fn complex(&mut self, t: &[Token]) -> Result<Complex, Reason> {
        let parts = self.compounds(trim_whitespace(t), Combinator::Descendant)?;
        Ok(right_to_left(parts))
    }

    /// A relative selector: an optional leading combinator, then a complex
    /// selector, anchored to the `:has()` subject.
    fn relative(&mut self, t: &[Token]) -> Result<Relative, Reason> {
        let t = trim_whitespace(t);
        let (leading, rest) = match t.first() {
            Some(Token::Delim('>')) => (Combinator::Child, &t[1..]),
            Some(Token::Delim('+')) => (Combinator::NextSibling, &t[1..]),
            Some(Token::Delim('~')) => (Combinator::SubsequentSibling, &t[1..]),
            _ => (Combinator::Descendant, t),
        };
        let mut parts = self.compounds(trim_whitespace(rest), leading)?;

        let rest_are_siblings = parts[1..].iter().all(|(_, c)| c.is_sibling());
        let reach = match leading {
            Combinator::Child if parts.len() == 1 || rest_are_siblings => Reach::Children,
            Combinator::Child | Combinator::Descendant => Reach::Descendants,
            Combinator::NextSibling if parts.len() == 1 => Reach::NextSibling,
            _ if rest_are_siblings => Reach::Siblings,
            _ => Reach::SiblingsAndDescendants,
        };

        parts.insert(0, (Box::new([Simple::Anchor]), Combinator::Descendant));
        Ok(Relative {
            complex: right_to_left(parts),
            reach,
        })
    }

    /// Compounds and combinators, left to right. Each compound comes with
    /// the combinator on its left; the first one gets `first`.
    fn compounds(&mut self, t: &[Token], first: Combinator) -> Result<Compounds, Reason> {
        if t.is_empty() {
            return Err("expected a selector");
        }
        let mut parts = Vec::new();
        let mut i = 0;
        let mut combinator = first;
        loop {
            let (compound, has_pseudo_element) = self.compound(t, &mut i)?;
            parts.push((compound, combinator));
            if i == t.len() {
                return Ok(parts);
            }
            if has_pseudo_element {
                return Err("a pseudo-element must come last");
            }
            combinator = parse_combinator(t, &mut i)?;
        }
    }

    /// A compound selector starting at `t[*i]`. Also returns whether it
    /// ends in a pseudo-element.
    fn compound(&mut self, t: &[Token], i: &mut usize) -> Result<(Box<[Simple]>, bool), Reason> {
        let start = *i;
        let mut simples = Vec::new();
        simples.extend(self.type_selector(t, i)?);
        let mut pseudo_element = false;
        while let Some(token) = t.get(*i) {
            let is_subclass = matches!(
                token,
                Token::Hash { .. } | Token::Delim('.') | Token::LBracket
            );
            if is_subclass && pseudo_element {
                return Err("a pseudo-element must come last");
            }
            match token {
                Token::Hash { value, id } => {
                    if !id {
                        return Err("an ID selector must be an identifier");
                    }
                    simples.push(Simple::Id(value.as_str().into()));
                    *i += 1;
                }
                Token::Delim('.') => match t.get(*i + 1) {
                    Some(Token::Ident(class)) => {
                        simples.push(Simple::Class(class.as_str().into()));
                        *i += 2;
                    }
                    _ => return Err("expected a class name after '.'"),
                },
                Token::LBracket => {
                    let (inner, next) = block(t, *i);
                    simples.push(Simple::Attr(Box::new(self.attribute(inner)?)));
                    *i = next;
                }
                Token::Colon => {
                    let (simple, is_pseudo_element) = match t.get(*i + 1) {
                        Some(Token::Colon) => (self.pseudo_element(t, i)?, true),
                        _ => self.pseudo_class(t, i)?,
                    };
                    if is_pseudo_element {
                        if self.nesting > 0 {
                            return Err("pseudo-elements are not allowed here");
                        }
                        pseudo_element = true;
                    }
                    simples.push(simple);
                }
                _ => break,
            }
        }
        if *i == start {
            return Err(match t.get(*i) {
                Some(Token::Comma) => "unexpected ','",
                _ => "expected a selector",
            });
        }
        Ok((simples.into(), pseudo_element))
    }

    /// A type selector or universal selector with an optional namespace
    /// prefix. `Ok(None)` either when there is none or when it matches every
    /// element (`*`, `*|*`).
    fn type_selector(&self, t: &[Token], i: &mut usize) -> Result<Option<Simple>, Reason> {
        let is_name = |token: Option<&Token>| {
            matches!(token, Some(Token::Ident(_)) | Some(Token::Delim('*')))
        };
        let first = t.get(*i);
        // (namespace prefix, local name token)
        let (prefix, local) = if is_name(first)
            && t.get(*i + 1) == Some(&Token::Delim('|'))
            && is_name(t.get(*i + 2))
        {
            *i += 3;
            (Some(first), &t[*i - 1])
        } else if first == Some(&Token::Delim('|')) && is_name(t.get(*i + 1)) {
            *i += 2;
            (None, &t[*i - 1])
        } else if is_name(first) {
            *i += 1;
            (Some(None), &t[*i - 1])
        } else {
            return Ok(None);
        };
        // Some(None): no prefix. Some(Some(*)): any namespace.
        // None: `|E`, no namespace.
        let no_namespace = match prefix {
            Some(None) | Some(Some(Token::Delim('*'))) => false,
            None => true,
            Some(Some(_)) => return Err("undeclared namespace prefix"),
        };
        let local = match local {
            Token::Ident(name) => Some(self.local_name(name)),
            _ => None,
        };
        if local.is_none() && !no_namespace {
            return Ok(None);
        }
        Ok(Some(Simple::Type {
            no_namespace,
            local,
        }))
    }

    /// The contents of `[...]`.
    fn attribute(&self, t: &[Token]) -> Result<AttrSelector, Reason> {
        use Token::{Delim, Ident};

        let t = trim_whitespace(t);
        let (any_namespace, name, mut i) = match (t.first(), t.get(1), t.get(2)) {
            (Some(Delim('*')), Some(Delim('|')), Some(Ident(name))) => (true, name, 3),
            (Some(Delim('|')), Some(Ident(name)), _) => (false, name, 2),
            (Some(Ident(_)), Some(Delim('|')), Some(Ident(_))) => {
                return Err("undeclared namespace prefix");
            }
            (Some(Ident(name)), _, _) => (false, name, 1),
            _ => return Err("expected an attribute name"),
        };
        let local = self.local_name(name);
        let case = if CASE_INSENSITIVE_ATTRS.contains(&name.to_ascii_lowercase().as_str()) {
            AttrCase::HtmlLegacy
        } else {
            AttrCase::Sensitive
        };
        skip_whitespace(t, &mut i);
        if i == t.len() {
            return Ok(AttrSelector {
                any_namespace,
                local,
                value: None,
                case,
            });
        }

        let op = match (&t[i], t.get(i + 1)) {
            (Delim('='), _) => AttrOp::Equals,
            (Delim(c), Some(Delim('='))) => {
                i += 1;
                match c {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::DashMatch,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    '*' => AttrOp::Substring,
                    _ => return Err("expected an attribute matcher"),
                }
            }
            _ => return Err("expected an attribute matcher"),
        };
        i += 1;
        skip_whitespace(t, &mut i);
        let value = match t.get(i) {
            Some(Ident(v) | Token::String(v)) => v.as_str().into(),
            _ => return Err("expected an attribute value"),
        };
        i += 1;
        skip_whitespace(t, &mut i);
        let case = match t.get(i) {
            Some(Ident(flag)) if flag.eq_ignore_ascii_case("i") => AttrCase::Insensitive,
            Some(Ident(flag)) if flag.eq_ignore_ascii_case("s") => AttrCase::Sensitive,
            None => case,
            _ => return Err("unexpected token in attribute selector"),
        };
        if i < t.len() {
            i += 1;
            skip_whitespace(t, &mut i);
            if i < t.len() {
                return Err("unexpected token in attribute selector");
            }
        }
        Ok(AttrSelector {
            any_namespace,
            local,
            value: Some((op, value)),
            case,
        })
    }

    /// `::name` or `::name(...)`, starting at the first colon.
    fn pseudo_element(&self, t: &[Token], i: &mut usize) -> Result<Simple, Reason> {
        match t.get(*i + 2) {
            Some(Token::Ident(name)) => {
                let name = name.to_ascii_lowercase();
                if !PSEUDO_ELEMENTS.contains(&name.as_str()) && !name.starts_with("-webkit-") {
                    return Err("unknown pseudo-element");
                }
                *i += 3;
            }
            Some(Token::Function(name)) => {
                if !PSEUDO_ELEMENT_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err("unknown pseudo-element");
                }
                *i = block(t, *i + 2).1;
            }
            _ => return Err("expected a pseudo-element name"),
        }
        Ok(Simple::Never)
    }

    /// `:name` or `:name(...)`, starting at the colon. Also returns whether
    /// it was really one of the legacy single-colon pseudo-elements.
    fn pseudo_class(&mut self, t: &[Token], i: &mut usize) -> Result<(Simple, bool), Reason> {
        let pseudo = |p| Ok((Simple::Pseudo(p), false));
        let nth = |a, b, from_end, of_type| {
            pseudo(Pseudo::Nth(Box::new(Nth {
                a,
                b,
                from_end,
                of_type,
                of: None,
            })))
        };

        let name = match t.get(*i + 1) {
            Some(Token::Ident(name)) => name.to_ascii_lowercase(),
            Some(Token::Function(name)) => {
                let name = name.to_ascii_lowercase();
                let (args, next) = block(t, *i + 1);
                *i = next;
                return self.functional_pseudo_class(&name, args);
            }
            _ => return Err("expected a pseudo-class name"),
        };
        *i += 2;
        match name.as_str() {
            "before" | "after" | "first-line" | "first-letter" => Ok((Simple::Never, true)),
            "root" => pseudo(Pseudo::Root),
            "empty" => pseudo(Pseudo::Empty),
            "scope" => pseudo(Pseudo::Scope),
            "first-child" => nth(0, 1, false, false),
            "last-child" => nth(0, 1, true, false),
            "only-child" => pseudo(Pseudo::OnlyChild),
            "first-of-type" => nth(0, 1, false, true),
            "last-of-type" => nth(0, 1, true, true),
            "only-of-type" => pseudo(Pseudo::OnlyOfType),
            "any-link" | "link" | "-webkit-any-link" => pseudo(Pseudo::AnyLink),
            "enabled" => pseudo(Pseudo::Enabled),
            "disabled" => pseudo(Pseudo::Disabled),
            "checked" => pseudo(Pseudo::Checked),
            "indeterminate" => pseudo(Pseudo::Indeterminate),
            "default" => pseudo(Pseudo::Default),
            "required" => pseudo(Pseudo::Required),
            "optional" => pseudo(Pseudo::Optional),
            "read-only" => pseudo(Pseudo::ReadOnly),
            "read-write" => pseudo(Pseudo::ReadWrite),
            "placeholder-shown" => pseudo(Pseudo::PlaceholderShown),
            "open" => pseudo(Pseudo::Open),
            "defined" => pseudo(Pseudo::Defined),
            "visible" => pseudo(Pseudo::Visible),
            name if NEVER_PSEUDO_CLASSES.contains(&name) => Ok((Simple::Never, false)),
            _ => Err("unknown pseudo-class"),
        }
    }

    fn functional_pseudo_class(
        &mut self,
        name: &str,
        args: &[Token],
    ) -> Result<(Simple, bool), Reason> {
        let pseudo = match name {
            "not" | "is" | "where" | "has" => {
                self.nesting += 1;
                let result = match name {
                    "not" => self.selector_list(args, false).map(Pseudo::Not),
                    "has" => self.relative_list(args).map(Pseudo::Has),
                    _ => self.selector_list(args, true).map(Pseudo::Is),
                };
                self.nesting -= 1;
                result?
            }
            "nth-child" | "nth-last-child" => {
                let (anb, of) = match args
                    .iter()
                    .position(|t| matches!(t, Token::Ident(s) if s.eq_ignore_ascii_case("of")))
                {
                    Some(at) => {
                        self.nesting += 1;
                        let of = self.selector_list(&args[at + 1..], false);
                        self.nesting -= 1;
                        (&args[..at], Some(of?))
                    }
                    None => (args, None),
                };
                let (a, b) = parse_anb(anb).ok_or("invalid An+B expression")?;
                Pseudo::Nth(Box::new(Nth {
                    a,
                    b,
                    from_end: name == "nth-last-child",
                    of_type: false,
                    of,
                }))
            }
            "nth-of-type" | "nth-last-of-type" => {
                let (a, b) = parse_anb(args).ok_or("invalid An+B expression")?;
                Pseudo::Nth(Box::new(Nth {
                    a,
                    b,
                    from_end: name == "nth-last-of-type",
                    of_type: true,
                    of: None,
                }))
            }
            "lang" => {
                let ranges = split_commas(args)
                    .into_iter()
                    .map(|item| match trim_whitespace(item) {
                        [Token::Ident(s) | Token::String(s)] => Ok(s.as_str().into()),
                        _ => Err("expected a language range"),
                    })
                    .collect::<Result<_, _>>()?;
                Pseudo::Lang(ranges)
            }
            "dir" => match trim_whitespace(args) {
                [Token::Ident(dir)] => Pseudo::Dir(match dir.to_ascii_lowercase().as_str() {
                    "ltr" => Some(false),
                    "rtl" => Some(true),
                    _ => None,
                }),
                _ => return Err("expected a direction"),
            },
            name if NEVER_PSEUDO_CLASS_FUNCTIONS.contains(&name) => {
                return Ok((Simple::Never, false));
            }
            _ => return Err("unknown pseudo-class"),
        };
        Ok((Simple::Pseudo(pseudo), false))
    }
}

/// A combinator between two compounds, with the whitespace around it.
fn parse_combinator(t: &[Token], i: &mut usize) -> Result<Combinator, Reason> {
    let had_whitespace = skip_whitespace(t, i);
    let combinator = match t.get(*i) {
        Some(Token::Delim('>')) => Combinator::Child,
        Some(Token::Delim('+')) => Combinator::NextSibling,
        Some(Token::Delim('~')) => Combinator::SubsequentSibling,
        Some(_) if had_whitespace => return Ok(Combinator::Descendant),
        Some(Token::Comma) => return Err("unexpected ','"),
        _ => return Err("unexpected token"),
    };
    *i += 1;
    skip_whitespace(t, i);
    if *i == t.len() {
        return Err("expected a selector after the combinator");
    }
    Ok(combinator)
}

/// Turns left-to-right (compound, combinator on its left) pairs into a
/// [`Complex`].
fn right_to_left(parts: Compounds) -> Complex {
    Complex {
        parts: parts
            .into_iter()
            .rev()
            .map(|(compound, combinator)| Part {
                compound,
                combinator,
            })
            .collect(),
    }
}

/// Parses the An+B microsyntax (CSS Syntax §6). `+ n` is not allowed to
/// have whitespace after the `+`; whitespace around the sign of B is.
/// Values saturate to the `i32` range.
fn parse_anb(t: &[Token]) -> Option<(i32, i32)> {
    /// What follows the `n` in the token that contains it.
    enum Rest {
        /// Just `n`: B, if any, is in the following tokens.
        Nothing,
        /// `n-`: an unsigned integer must follow.
        Dash,
        /// `n-<digits>`: B is complete.
        B(i64),
    }

    fn rest_after_n(s: &str) -> Option<Rest> {
        match s {
            "" => Some(Rest::Nothing),
            "-" => Some(Rest::Dash),
            _ => {
                let digits = s.strip_prefix('-')?;
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    Some(Rest::B(-digits.parse::<i64>().unwrap_or(i64::MAX)))
                } else {
                    None
                }
            }
        }
    }

    fn unsigned_int(token: Option<&Token>) -> Option<i64> {
        match token {
            Some(Token::Number(n)) if !n.signed => n.as_int(),
            _ => None,
        }
    }

    let t = trim_whitespace(t);
    let mut i = 1;
    let (a, rest) = match t.first()? {
        Token::Number(n) => (0, Rest::B(n.as_int()?)),
        Token::Dimension(n, unit) => {
            let unit = unit.to_ascii_lowercase();
            (n.as_int()?, rest_after_n(unit.strip_prefix('n')?)?)
        }
        Token::Ident(s) => {
            let s = s.to_ascii_lowercase();
            match s.as_str() {
                "odd" => (2, Rest::B(1)),
                "even" => (2, Rest::B(0)),
                _ => match s.strip_prefix("-n") {
                    Some(rest) => (-1, rest_after_n(rest)?),
                    None => (1, rest_after_n(s.strip_prefix('n')?)?),
                },
            }
        }
        Token::Delim('+') => match t.get(1)? {
            Token::Ident(s) => {
                i = 2;
                let s = s.to_ascii_lowercase();
                (1, rest_after_n(s.strip_prefix('n')?)?)
            }
            _ => return None,
        },
        _ => return None,
    };
    let b = match rest {
        Rest::B(b) => b,
        Rest::Dash => {
            skip_whitespace(t, &mut i);
            let b = unsigned_int(t.get(i))?;
            i += 1;
            -b
        }
        Rest::Nothing => {
            skip_whitespace(t, &mut i);
            match t.get(i) {
                None => 0,
                Some(Token::Number(n)) if n.signed => {
                    i += 1;
                    n.as_int()?
                }
                Some(Token::Delim(sign @ ('+' | '-'))) => {
                    i += 1;
                    skip_whitespace(t, &mut i);
                    let b = unsigned_int(t.get(i))?;
                    i += 1;
                    if *sign == '-' { -b } else { b }
                }
                Some(_) => return None,
            }
        }
    };
    let clamp = |v: i64| v.clamp(i32::MIN.into(), i32::MAX.into()) as i32;
    (i == t.len()).then(|| (clamp(a), clamp(b)))
}

/// The tokens inside the block opened at `t[open]`, and the index after
/// its closing token. A block left open runs to the end of the input.
fn block(t: &[Token], open: usize) -> (&[Token], usize) {
    let mut closers = Vec::new();
    for (i, token) in t.iter().enumerate().skip(open) {
        match token {
            Token::Function(_) | Token::LParen => closers.push(Token::RParen),
            Token::LBracket => closers.push(Token::RBracket),
            Token::LBrace => closers.push(Token::RBrace),
            token if Some(token) == closers.last() => {
                closers.pop();
                if closers.is_empty() {
                    return (&t[open + 1..i], i + 1);
                }
            }
            _ => {}
        }
    }
    (&t[open + 1..], t.len())
}

/// Splits `t` at the commas that are not inside a block.
fn split_commas(t: &[Token]) -> Vec<&[Token]> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            Token::Comma => {
                items.push(&t[start..i]);
                start = i + 1;
                i += 1;
            }
            Token::Function(_) | Token::LParen | Token::LBracket | Token::LBrace => {
                i = block(t, i).1;
            }
            _ => i += 1,
        }
    }
    items.push(&t[start..]);
    items
}

fn trim_whitespace(mut t: &[Token]) -> &[Token] {
    while let [Token::Whitespace, rest @ ..] = t {
        t = rest;
    }
    while let [rest @ .., Token::Whitespace] = t {
        t = rest;
    }
    t
}

/// Skips whitespace tokens. Returns whether there were any.
fn skip_whitespace(t: &[Token], i: &mut usize) -> bool {
    let start = *i;
    while t.get(*i) == Some(&Token::Whitespace) {
        *i += 1;
    }
    *i > start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anb(s: &str) -> Option<(i32, i32)> {
        parse_anb(&tokenize(s))
    }

    #[test]
    fn an_plus_b() {
        let cases = [
            ("odd", (2, 1)),
            ("EVEN", (2, 0)),
            ("5", (0, 5)),
            ("+5", (0, 5)),
            ("-5", (0, -5)),
            ("n", (1, 0)),
            ("+n", (1, 0)),
            ("-n", (-1, 0)),
            ("N", (1, 0)),
            ("2n", (2, 0)),
            ("2n+1", (2, 1)),
            ("2n-1", (2, -1)),
            ("-2n+3", (-2, 3)),
            ("2n + 1", (2, 1)),
            ("2n- 1", (2, -1)),
            ("2n -1", (2, -1)),
            ("2n - 1", (2, -1)),
            ("n-1", (1, -1)),
            ("-n-1", (-1, -1)),
            ("+n-1", (1, -1)),
            ("-n- 1", (-1, -1)),
            ("+n+ 1", (1, 1)),
            ("  3n+0  ", (3, 0)),
            ("0n", (0, 0)),
            ("99999999999n", (i32::MAX, 0)),
        ];
        for (input, expected) in cases {
            assert_eq!(anb(input), Some(expected), "{input:?}");
        }
        for input in [
            "", "+ n", "n +", "2n + +1", "2n + -1", "1.5n", "2.0", "n- -1", "a", "2n 1", "odd 1",
            "- n", "2n+1 x", "3 n", "--n",
        ] {
            assert_eq!(anb(input), None, "{input:?}");
        }
    }

    fn valid(s: &str) {
        if let Err(e) = parse(s) {
            panic!("{s:?} should parse: {e}");
        }
    }

    fn invalid(s: &str) {
        assert!(parse(s).is_err(), "{s:?} should not parse");
    }

    #[test]
    fn valid_selectors() {
        for s in [
            "*",
            "div",
            "DIV",
            "#id",
            ".a.b",
            "div#id.a[b]:first-child",
            "a > b + c ~ d e",
            "a>b+c~d",
            "  a  ,  b  ",
            "a,b,c",
            "*|div",
            "|div",
            "*|*",
            "|*",
            "[a]",
            "[ a ]",
            "[a=b]",
            "[a='b c']",
            "[a=\"b\" i]",
            "[a=b s]",
            "[a~=b][a|=b][a^=b][a$=b][a*=b]",
            "[*|a]",
            "[|a]",
            "[a|=b]",
            "[a=b I]",
            r"#\31 23",
            r".md\:flex",
            ":not(a, b > c)",
            ":is(a, b)",
            ":where(a)",
            ":is()",
            ":is(:::, a)",
            ":where(!)",
            ":is(::before)",
            ":has(> a)",
            ":has(+ a, ~ b c, d)",
            ":nth-child(2n+1)",
            ":nth-child(odd of .a, .b)",
            ":nth-last-child(-n+3)",
            ":nth-of-type(even)",
            ":nth-last-of-type(1)",
            ":first-child:last-child:only-child",
            ":first-of-type:last-of-type:only-of-type",
            ":root:empty:scope",
            ":hover:focus:active:visited:target:focus-within:focus-visible",
            ":any-link:link:enabled:disabled:checked:indeterminate:default",
            ":required:optional:read-only:read-write:placeholder-shown",
            ":open:defined:visible",
            ":lang(en, \"fr-CA\")",
            ":dir(rtl)",
            ":dir(sideways)",
            ":NOT(.a):Hover",
            "div::before",
            "div:before",
            "div::-webkit-scrollbar",
            "p::first-line:hover",
            "::slotted(span)",
            "a /* comment */ b",
            "a[href",
            "a:not(.b",
            "[title='unclosed",
            ":host",
            ":host(.a)",
            ":state(open)",
        ] {
            valid(s);
        }
    }

    #[test]
    fn invalid_selectors() {
        for s in [
            "",
            "   ",
            ",",
            "a,",
            ",a",
            "a,,b",
            "a >",
            "> a",
            "a > > b",
            "a +",
            "#1a",
            "#",
            ".",
            ".1a",
            "a.",
            "[]",
            "[1]",
            "[a=]",
            "[a==b]",
            "[a=b c]",
            "[a=b i i]",
            "[a=1]",
            "[ns|a]",
            "ns|div",
            "a|",
            ":",
            "::",
            ":nope",
            "::nope",
            ":not()",
            ":not(a,)",
            ":not(!)",
            ":has()",
            ":has(!)",
            ":nth-child()",
            ":nth-child(2n+)",
            ":nth-child(of a)",
            ":nth-child(1 of)",
            ":nth-of-type(1 of a)",
            ":lang()",
            ":dir()",
            ":dir(ltr rtl)",
            "div::before span",
            "div::before.a",
            ":not(::before)",
            "a || b",
            "a ! b",
            ")",
            "a)",
            "]",
            "{}",
            "@media",
            "a{",
            "<!--",
            "a 'b'",
            "a'b",
        ] {
            invalid(s);
        }
    }

    #[test]
    fn forgiving_lists_drop_invalid_items() {
        let list = parse(":is(a, !, b)").unwrap();
        let Simple::Pseudo(Pseudo::Is(inner)) = &list.selectors[0].parts[0].compound[0] else {
            panic!("expected :is()");
        };
        assert_eq!(inner.selectors.len(), 2);
    }

    #[test]
    fn complex_selectors_are_stored_right_to_left() {
        let list = parse("a > b c").unwrap();
        let parts = &list.selectors[0].parts;
        let combinators: Vec<_> = parts.iter().map(|p| p.combinator).collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(combinators[0], Combinator::Descendant);
        assert_eq!(combinators[1], Combinator::Child);
        assert!(matches!(
            &parts[2].compound[0],
            Simple::Type { local: Some(l), .. } if l.exact == NameRef::Static(dasher_dom::Name::A)
        ));
    }

    #[test]
    fn has_reach() {
        let reach = |s: &str| {
            let list = parse(&format!(":has({s})")).unwrap();
            let Simple::Pseudo(Pseudo::Has(rel)) = &list.selectors[0].parts[0].compound[0] else {
                panic!("expected :has()");
            };
            rel[0].reach
        };
        assert_eq!(reach("> a"), Reach::Children);
        assert_eq!(reach("> a + b"), Reach::Children);
        assert_eq!(reach("> a b"), Reach::Descendants);
        assert_eq!(reach("a"), Reach::Descendants);
        assert_eq!(reach("+ a"), Reach::NextSibling);
        assert_eq!(reach("+ a ~ b"), Reach::Siblings);
        assert_eq!(reach("~ a"), Reach::Siblings);
        assert_eq!(reach("~ a > b"), Reach::SiblingsAndDescendants);
    }

    #[test]
    fn errors_name_the_selector() {
        assert!(
            parse("a[href").is_ok(),
            "an unclosed block is closed at the end"
        );
        assert_eq!(
            parse("div[").unwrap_err().reason(),
            "expected an attribute name"
        );
        let err = parse("div >").unwrap_err();
        assert_eq!(err.name(), "SyntaxError");
        assert_eq!(err.selector(), "div >");
        assert_eq!(
            err.to_string(),
            "'div >' is not a valid selector: expected a selector after the combinator"
        );
    }
}
