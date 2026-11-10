//! Media query evaluation for `matchMedia`, against the configured viewport.
//!
//! Nothing is rendered, so the answers are static: dimensions come from the
//! [`Viewport`], and user preferences are those of a default desktop browser
//! (light colour scheme, no reduced motion, a fine pointer that can hover
//! unless the viewport is a touch device). Queries that fail to parse do
//! not match, like `not all`.
//! <https://drafts.csswg.org/mediaqueries-4/>

use rquickjs::{Ctx, Function, Object};

use crate::window::Viewport;

/// What a media query is evaluated against.
#[derive(Clone, Debug)]
pub(crate) struct MediaEnv {
    width: f64,
    height: f64,
    screen_width: f64,
    screen_height: f64,
    dpr: f64,
    touch: bool,
}

impl MediaEnv {
    pub(crate) fn new(viewport: &Viewport) -> Self {
        Self {
            width: f64::from(viewport.width),
            height: f64::from(viewport.height),
            screen_width: f64::from(viewport.screen_width),
            screen_height: f64::from(viewport.screen_height),
            dpr: viewport.device_pixel_ratio,
            touch: viewport.touch,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    /// A number and its unit (empty when unitless).
    Number(f64, String),
    Open,
    Close,
    Comma,
    Colon,
    Slash,
    /// `<`, `<=`, `>`, `>=` or `=`.
    Op(Op),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl Op {
    fn flip(self) -> Self {
        match self {
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
            Self::Eq => Self::Eq,
        }
    }

    fn holds(self, a: f64, b: f64) -> bool {
        const EPSILON: f64 = 1e-9;
        match self {
            Self::Lt => a < b - EPSILON,
            Self::Le => a <= b + EPSILON,
            Self::Gt => a > b + EPSILON,
            Self::Ge => a >= b - EPSILON,
            Self::Eq => (a - b).abs() <= EPSILON,
        }
    }
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = input.to_ascii_lowercase().chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '<' | '>' => {
                let eq = next == Some('=');
                tokens.push(Token::Op(match (c, eq) {
                    ('<', false) => Op::Lt,
                    ('<', true) => Op::Le,
                    ('>', false) => Op::Gt,
                    _ => Op::Ge,
                }));
                i += if eq { 2 } else { 1 };
            }
            '=' => {
                tokens.push(Token::Op(Op::Eq));
                i += 1;
            }
            c if c.is_ascii_digit()
                || (c == '.' && next.is_some_and(|n| n.is_ascii_digit()))
                || ((c == '+' || c == '-')
                    && next.is_some_and(|n| n.is_ascii_digit() || n == '.')) =>
            {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let number: String = chars[start..i].iter().collect();
                let unit_start = i;
                while i < chars.len() && (chars[i].is_ascii_alphabetic() || chars[i] == '%') {
                    i += 1;
                }
                let unit: String = chars[unit_start..i].iter().collect();
                tokens.push(Token::Number(number.parse().ok()?, unit));
            }
            c if c.is_ascii_alphabetic() || c == '-' || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '-' || chars[i] == '_')
                {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            _ => return None,
        }
    }
    Some(tokens)
}

/// A feature's current value.
enum Actual {
    /// A length in CSS pixels.
    Length(f64),
    /// A ratio, as a quotient.
    Ratio(f64),
    /// A resolution in dots per CSS pixel.
    Resolution(f64),
    /// A plain number.
    Number(f64),
    /// A keyword.
    Keyword(&'static str),
}

impl MediaEnv {
    fn actual(&self, name: &str) -> Option<Actual> {
        let pointer = if self.touch { "coarse" } else { "fine" };
        let hover = if self.touch { "none" } else { "hover" };
        Some(match name {
            "width" => Actual::Length(self.width),
            "height" => Actual::Length(self.height),
            "device-width" => Actual::Length(self.screen_width),
            "device-height" => Actual::Length(self.screen_height),
            "aspect-ratio" => Actual::Ratio(self.width / self.height),
            "device-aspect-ratio" => Actual::Ratio(self.screen_width / self.screen_height),
            "resolution" => Actual::Resolution(self.dpr),
            "-webkit-device-pixel-ratio" => Actual::Number(self.dpr),
            "color" => Actual::Number(8.0),
            "color-index" | "monochrome" | "grid" => Actual::Number(0.0),
            "orientation" => Actual::Keyword(if self.height >= self.width {
                "portrait"
            } else {
                "landscape"
            }),
            "hover" | "any-hover" => Actual::Keyword(hover),
            "pointer" | "any-pointer" => Actual::Keyword(pointer),
            "prefers-color-scheme" => Actual::Keyword("light"),
            "prefers-reduced-motion"
            | "prefers-reduced-transparency"
            | "prefers-contrast"
            | "prefers-reduced-data" => Actual::Keyword("no-preference"),
            "forced-colors" | "inverted-colors" => Actual::Keyword("none"),
            "color-gamut" => Actual::Keyword("srgb"),
            "dynamic-range" | "video-dynamic-range" => Actual::Keyword("standard"),
            "display-mode" => Actual::Keyword("browser"),
            "scripting" => Actual::Keyword("enabled"),
            "update" => Actual::Keyword("fast"),
            "overflow-block" | "overflow-inline" => Actual::Keyword("scroll"),
            _ => return None,
        })
    }

    /// Converts a length to CSS pixels.
    fn length(&self, value: f64, unit: &str) -> Option<f64> {
        let px = match unit {
            "" if value == 0.0 => 0.0,
            "px" => value,
            "em" | "rem" => value * 16.0,
            "ex" | "ch" => value * 8.0,
            "vw" => value * self.width / 100.0,
            "vh" => value * self.height / 100.0,
            "vmin" => value * self.width.min(self.height) / 100.0,
            "vmax" => value * self.width.max(self.height) / 100.0,
            "in" => value * 96.0,
            "cm" => value * 96.0 / 2.54,
            "mm" => value * 96.0 / 25.4,
            "q" => value * 96.0 / 101.6,
            "pt" => value * 96.0 / 72.0,
            "pc" => value * 16.0,
            _ => return None,
        };
        Some(px)
    }
}

fn resolution(value: f64, unit: &str) -> Option<f64> {
    match unit {
        "dppx" | "x" => Some(value),
        "dpi" => Some(value / 96.0),
        "dpcm" => Some(value * 2.54 / 96.0),
        _ => None,
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    env: &'a MediaEnv,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_ident(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Ident(name)) => Some(name),
            _ => None,
        }
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_ident(&mut self, word: &str) -> bool {
        if self.peek_ident() == Some(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// A query: `[not|only] type [and condition]`, or a bare condition.
    fn query(&mut self) -> Option<bool> {
        if self.peek() == Some(&Token::Open)
            || self.peek_ident() == Some("not")
                && self.tokens.get(self.pos + 1) == Some(&Token::Open)
        {
            return self.condition(true);
        }
        let negate = self.eat_ident("not");
        if !negate {
            self.eat_ident("only");
        }
        let media_type = match self.peek() {
            Some(Token::Ident(name)) => name.clone(),
            _ => return None,
        };
        self.pos += 1;
        let type_matches = match media_type.as_str() {
            "all" | "screen" => true,
            "and" | "or" | "not" | "only" | "layer" => return None,
            _ => false,
        };
        let mut result = type_matches;
        if self.eat_ident("and") {
            let condition = self.condition(false)?;
            result = result && condition;
        }
        Some(result != negate)
    }

    /// `not <in-parens>`, or in-parens joined by all `and` or all `or`.
    fn condition(&mut self, allow_or: bool) -> Option<bool> {
        if self.eat_ident("not") {
            return Some(!self.in_parens()?);
        }
        let mut result = self.in_parens()?;
        let joiner = match self.peek_ident() {
            Some("and") => "and",
            Some("or") if allow_or => "or",
            _ => return Some(result),
        };
        while self.eat_ident(joiner) {
            let next = self.in_parens()?;
            result = if joiner == "and" {
                result && next
            } else {
                result || next
            };
        }
        Some(result)
    }

    fn in_parens(&mut self) -> Option<bool> {
        if !self.eat(&Token::Open) {
            return None;
        }
        let nested = self.peek() == Some(&Token::Open)
            || (self.peek_ident() == Some("not")
                && self.tokens.get(self.pos + 1) == Some(&Token::Open));
        let result = if nested {
            self.condition(true)?
        } else {
            self.feature()?
        };
        self.eat(&Token::Close).then_some(result)
    }

    /// A value token sequence: a number, a ratio or a keyword.
    fn value(&mut self) -> Option<Token> {
        let token = self.peek()?.clone();
        match token {
            Token::Number(n, ref unit) if unit.is_empty() => {
                self.pos += 1;
                if self.eat(&Token::Slash) {
                    match self.peek() {
                        Some(Token::Number(d, unit)) if unit.is_empty() => {
                            let d = *d;
                            self.pos += 1;
                            return Some(Token::Number(n / d, String::from("/")));
                        }
                        _ => return None,
                    }
                }
                Some(token)
            }
            Token::Number(..) | Token::Ident(_) => {
                self.pos += 1;
                Some(token)
            }
            _ => None,
        }
    }

    fn feature(&mut self) -> Option<bool> {
        // Range syntax starting with a value: `600px <= width [< 900px]`.
        // Keywords are never compared by range, so a leading value is a number.
        if !matches!(self.peek(), Some(Token::Ident(_))) {
            let left = self.value()?;
            let Some(Token::Op(op1)) = self.peek().cloned() else {
                return None;
            };
            self.pos += 1;
            let name = self.peek_ident()?.to_owned();
            self.pos += 1;
            let first = self.compare(&name, &left, op1.flip())?;
            if let Some(Token::Op(op2)) = self.peek().cloned() {
                self.pos += 1;
                let right = self.value()?;
                let second = self.compare(&name, &right, op2)?;
                return Some(first && second);
            }
            return Some(first);
        }

        let name = self.peek_ident()?.to_owned();
        self.pos += 1;
        match self.peek().cloned() {
            Some(Token::Close) => self.boolean(&name),
            Some(Token::Colon) => {
                self.pos += 1;
                let value = self.value()?;
                if let Some(base) = name.strip_prefix("min-") {
                    self.compare(base, &value, Op::Ge)
                } else if let Some(base) = name.strip_prefix("max-") {
                    self.compare(base, &value, Op::Le)
                } else if let Some(rest) = name.strip_prefix("-webkit-min-") {
                    self.compare(&format!("-webkit-{rest}"), &value, Op::Ge)
                } else if let Some(rest) = name.strip_prefix("-webkit-max-") {
                    self.compare(&format!("-webkit-{rest}"), &value, Op::Le)
                } else {
                    self.compare(&name, &value, Op::Eq)
                }
            }
            Some(Token::Op(op)) => {
                self.pos += 1;
                let value = self.value()?;
                self.compare(&name, &value, op)
            }
            _ => None,
        }
    }

    /// Evaluates `(name)`. Unknown features make the query invalid, as in
    /// browsers, which treat such queries as `not all`.
    fn boolean(&self, name: &str) -> Option<bool> {
        Some(match self.env.actual(name)? {
            Actual::Length(v) | Actual::Ratio(v) | Actual::Resolution(v) | Actual::Number(v) => {
                v != 0.0
            }
            Actual::Keyword(k) => k != "none" && k != "no-preference",
        })
    }

    /// Compares the feature's actual value with `value` as
    /// `actual <op> value`. `None` means the value has the wrong type for
    /// the feature or the feature is unknown, which makes the whole query
    /// invalid.
    fn compare(&self, name: &str, value: &Token, op: Op) -> Option<bool> {
        let actual = self.env.actual(name)?;
        let (actual, expected) = match (actual, value) {
            (Actual::Length(a), Token::Number(n, unit)) => (a, self.env.length(*n, unit)?),
            (Actual::Ratio(a), Token::Number(n, unit)) if unit.is_empty() || unit == "/" => (a, *n),
            (Actual::Resolution(a), Token::Number(n, unit)) => (a, resolution(*n, unit)?),
            (Actual::Number(a), Token::Number(n, unit)) if unit.is_empty() => (a, *n),
            (Actual::Keyword(k), Token::Ident(word)) if op == Op::Eq => {
                return Some(k == word);
            }
            _ => return None,
        };
        Some(op.holds(actual, expected))
    }
}

/// Whether `query` (a comma-separated media query list) matches.
pub(crate) fn matches(query: &str, env: &MediaEnv) -> bool {
    let Some(tokens) = tokenize(query) else {
        return false;
    };
    if tokens.is_empty() {
        // The empty list matches everything.
        return true;
    }
    tokens.split(|t| *t == Token::Comma).any(|part| {
        let mut parser = Parser {
            tokens: part,
            pos: 0,
            env,
        };
        parser.query().filter(|_| parser.pos == part.len()) == Some(true)
    })
}

/// Adds the media query host function to the prelude's `native` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    viewport: &Viewport,
) -> rquickjs::Result<()> {
    let env = MediaEnv::new(viewport);
    native.set(
        "matchMedia",
        Function::new(ctx.clone(), move |query: String| matches(&query, &env))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> MediaEnv {
        MediaEnv::new(&Viewport::default())
    }

    fn check(env: &MediaEnv, cases: &[(&str, bool)]) {
        for &(query, expected) in cases {
            assert_eq!(matches(query, env), expected, "{query}");
        }
    }

    #[test]
    fn types_and_lists() {
        check(
            &desktop(),
            &[
                ("", true),
                ("all", true),
                ("screen", true),
                ("only screen", true),
                ("print", false),
                ("not print", true),
                ("not screen", false),
                ("print, screen", true),
                ("SCREEN", true),
                ("tv", false),
            ],
        );
    }

    #[test]
    fn dimensions() {
        check(
            &desktop(),
            &[
                ("(min-width: 1024px)", true),
                ("(max-width: 767px)", false),
                (
                    "screen and (min-width: 768px) and (max-width: 1919px)",
                    false,
                ),
                (
                    "screen and (min-width: 768px) and (max-width: 1920px)",
                    true,
                ),
                ("(width: 1920px)", true),
                ("(min-width: 120em)", true),
                ("(min-width: 121em)", false),
                ("(min-height: 50vh)", true),
                ("(width >= 600px)", true),
                ("(width < 600px)", false),
                ("(400px <= width <= 2000px)", true),
                ("(400px < width < 1000px)", false),
                ("(1000px > width)", false),
                ("(min-aspect-ratio: 16/9)", true),
                ("(aspect-ratio: 16 / 9)", true),
                ("(max-aspect-ratio: 4/3)", false),
                ("(orientation: landscape)", true),
                ("(orientation: portrait)", false),
                ("(min-device-width: 1920px)", true),
            ],
        );
    }

    #[test]
    fn resolution_and_pixel_ratio() {
        check(
            &desktop(),
            &[
                ("(min-resolution: 2dppx)", false),
                ("(min-resolution: 1dppx)", true),
                ("(min-resolution: 96dpi)", true),
                ("(min-resolution: 192dpi)", false),
                ("(-webkit-min-device-pixel-ratio: 2)", false),
                ("(-webkit-device-pixel-ratio: 1)", true),
                ("(resolution: 1x)", true),
            ],
        );
    }

    #[test]
    fn preferences_and_input() {
        let desktop = desktop();
        check(
            &desktop,
            &[
                ("(prefers-color-scheme: dark)", false),
                ("(prefers-color-scheme: light)", true),
                ("(prefers-reduced-motion: reduce)", false),
                ("(prefers-reduced-motion)", false),
                ("(prefers-reduced-motion: no-preference)", true),
                ("(hover: hover)", true),
                ("(hover)", true),
                ("(pointer: fine)", true),
                ("(pointer: coarse)", false),
                ("(any-pointer: coarse)", false),
                ("(color)", true),
                ("(monochrome)", false),
                ("(display-mode: standalone)", false),
                ("(forced-colors: active)", false),
                ("(scripting: enabled)", true),
            ],
        );
        let phone = MediaEnv::new(&Viewport {
            width: 390,
            height: 844,
            device_pixel_ratio: 3.0,
            screen_width: 390,
            screen_height: 844,
            touch: true,
        });
        check(
            &phone,
            &[
                ("(hover: none) and (pointer: coarse)", true),
                ("(max-width: 767px)", true),
                ("(orientation: portrait)", true),
                ("(-webkit-min-device-pixel-ratio: 2)", true),
            ],
        );
    }

    #[test]
    fn conditions() {
        check(
            &desktop(),
            &[
                ("not (max-width: 600px)", true),
                ("(max-width: 600px) or (min-width: 1000px)", true),
                ("((max-width: 600px) or (hover)) and (color)", true),
                ("not ((min-width: 100px) and (color))", false),
                ("screen and (not (color))", false),
            ],
        );
    }

    #[test]
    fn invalid_and_unknown_queries_do_not_match() {
        check(
            &desktop(),
            &[
                ("(min-width: 100)", false),
                ("(min-width: abc)", false),
                ("(width >)", false),
                ("screen and", false),
                ("(color", false),
                ("@@", false),
                ("(unknown-feature)", false),
                ("(unknown-feature: 1px)", false),
                ("not (unknown-feature)", false),
                (
                    "(min-width: 100px) and (max-width: 200px) or (color)",
                    false,
                ),
                ("garbage, screen", true),
            ],
        );
    }
}
