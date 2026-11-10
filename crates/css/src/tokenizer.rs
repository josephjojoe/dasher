//! A CSS tokenizer, as specified by CSS Syntax Level 3 (§4).
//!
//! Selectors are short, so the whole input is tokenized up front into a
//! `Vec<Token>`. Comments are dropped; whitespace is kept as a token, because
//! it is the descendant combinator. `url(` is tokenized as an ordinary
//! function token: selectors never contain URLs, and either way the result
//! is an invalid selector.

/// A numeric value with the flags the An+B microsyntax needs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Number {
    pub(crate) value: f64,
    /// The source had no fraction or exponent.
    pub(crate) integer: bool,
    /// The source started with `+` or `-`.
    pub(crate) signed: bool,
}

impl Number {
    /// The value as an integer, if it was written as one. Out-of-range
    /// values saturate.
    pub(crate) fn as_int(&self) -> Option<i64> {
        self.integer.then_some(self.value as i64)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Token {
    Ident(String),
    Function(String),
    AtKeyword(String),
    /// `#name`. `id` is set when the name is a valid identifier, which is
    /// what an ID selector needs (`#123` is not one).
    Hash {
        value: String,
        id: bool,
    },
    String(String),
    BadString,
    Delim(char),
    Number(Number),
    Percentage(Number),
    Dimension(Number, String),
    Whitespace,
    Cdo,
    Cdc,
    Colon,
    Semicolon,
    Comma,
    LBracket,
    RBracket,
    LParen,
    RParen,
    LBrace,
    RBrace,
}

/// Tokenizes `input`.
pub(crate) fn tokenize(input: &str) -> Vec<Token> {
    let mut t = Tokenizer {
        chars: preprocess(input),
        pos: 0,
    };
    let mut tokens = Vec::new();
    while let Some(token) = t.next_token() {
        tokens.push(token);
    }
    tokens
}

/// Input preprocessing (§3.3): newlines normalized to `\n`, NUL replaced.
fn preprocess(input: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\x0C' => out.push('\n'),
            '\0' => out.push(char::REPLACEMENT_CHARACTER),
            c => out.push(c),
        }
    }
    out
}

fn is_whitespace(c: char) -> bool {
    matches!(c, '\n' | '\t' | ' ')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_name(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-'
}

struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn next_token(&mut self) -> Option<Token> {
        self.skip_comments();
        let c = self.peek(0)?;
        self.pos += 1;
        Some(match c {
            c if is_whitespace(c) => {
                while self.peek(0).is_some_and(is_whitespace) {
                    self.pos += 1;
                }
                Token::Whitespace
            }
            '"' | '\'' => self.consume_string(c),
            '#' => {
                if self.peek(0).is_some_and(is_name) || self.valid_escape_at(0) {
                    let id = self.starts_ident_at(0);
                    Token::Hash {
                        value: self.consume_name(),
                        id,
                    }
                } else {
                    Token::Delim('#')
                }
            }
            '(' => Token::LParen,
            ')' => Token::RParen,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '+' | '.' => {
                self.pos -= 1;
                if self.starts_number_at(0) {
                    self.consume_numeric()
                } else {
                    self.pos += 1;
                    Token::Delim(c)
                }
            }
            '-' => {
                self.pos -= 1;
                if self.starts_number_at(0) {
                    self.consume_numeric()
                } else if self.peek(1) == Some('-') && self.peek(2) == Some('>') {
                    self.pos += 3;
                    Token::Cdc
                } else if self.starts_ident_at(0) {
                    self.consume_ident_like()
                } else {
                    self.pos += 1;
                    Token::Delim('-')
                }
            }
            '<' => {
                if self.peek(0) == Some('!')
                    && self.peek(1) == Some('-')
                    && self.peek(2) == Some('-')
                {
                    self.pos += 3;
                    Token::Cdo
                } else {
                    Token::Delim('<')
                }
            }
            '@' => {
                if self.starts_ident_at(0) {
                    Token::AtKeyword(self.consume_name())
                } else {
                    Token::Delim('@')
                }
            }
            '\\' => {
                self.pos -= 1;
                if self.valid_escape_at(0) {
                    self.consume_ident_like()
                } else {
                    self.pos += 1;
                    Token::Delim('\\')
                }
            }
            c if c.is_ascii_digit() => {
                self.pos -= 1;
                self.consume_numeric()
            }
            c if is_name_start(c) => {
                self.pos -= 1;
                self.consume_ident_like()
            }
            c => Token::Delim(c),
        })
    }

    fn skip_comments(&mut self) {
        while self.peek(0) == Some('/') && self.peek(1) == Some('*') {
            self.pos += 2;
            loop {
                match self.peek(0) {
                    None => return,
                    Some('*') if self.peek(1) == Some('/') => {
                        self.pos += 2;
                        break;
                    }
                    Some(_) => self.pos += 1,
                }
            }
        }
    }

    /// Whether the two characters at `offset` are a valid escape (§4.3.8).
    fn valid_escape_at(&self, offset: usize) -> bool {
        self.peek(offset) == Some('\\') && self.peek(offset + 1).is_some_and(|c| c != '\n')
    }

    /// Whether the three characters at `offset` would start an identifier
    /// (§4.3.9).
    fn starts_ident_at(&self, offset: usize) -> bool {
        match self.peek(offset) {
            Some('-') => {
                self.peek(offset + 1)
                    .is_some_and(|c| is_name_start(c) || c == '-')
                    || self.valid_escape_at(offset + 1)
            }
            Some('\\') => self.valid_escape_at(offset),
            Some(c) => is_name_start(c),
            None => false,
        }
    }

    /// Whether the three characters at `offset` would start a number
    /// (§4.3.10).
    fn starts_number_at(&self, offset: usize) -> bool {
        let digit = |o| self.peek(o).is_some_and(|c: char| c.is_ascii_digit());
        match self.peek(offset) {
            Some('+' | '-') => {
                digit(offset + 1) || (self.peek(offset + 1) == Some('.') && digit(offset + 2))
            }
            Some('.') => digit(offset + 1),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
    }

    /// Consumes an escape; the backslash has already been consumed (§4.3.7).
    fn consume_escape(&mut self) -> char {
        let Some(c) = self.peek(0) else {
            return char::REPLACEMENT_CHARACTER;
        };
        self.pos += 1;
        if !c.is_ascii_hexdigit() {
            return c;
        }
        let mut value = c.to_digit(16).expect("is a hex digit");
        for _ in 0..5 {
            match self.peek(0).and_then(|c| c.to_digit(16)) {
                Some(d) => {
                    value = value * 16 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        if self.peek(0).is_some_and(is_whitespace) {
            self.pos += 1;
        }
        match char::from_u32(value) {
            Some(c) if value != 0 => c,
            _ => char::REPLACEMENT_CHARACTER,
        }
    }

    /// Consumes a name (§4.3.12).
    fn consume_name(&mut self) -> String {
        let mut name = String::new();
        loop {
            match self.peek(0) {
                Some(c) if is_name(c) => {
                    name.push(c);
                    self.pos += 1;
                }
                Some('\\') if self.valid_escape_at(0) => {
                    self.pos += 1;
                    name.push(self.consume_escape());
                }
                _ => return name,
            }
        }
    }

    fn consume_ident_like(&mut self) -> Token {
        let name = self.consume_name();
        if self.peek(0) == Some('(') {
            self.pos += 1;
            Token::Function(name)
        } else {
            Token::Ident(name)
        }
    }

    fn consume_string(&mut self, quote: char) -> Token {
        let mut s = String::new();
        loop {
            match self.peek(0) {
                // EOF closes the string.
                None => return Token::String(s),
                Some(c) if c == quote => {
                    self.pos += 1;
                    return Token::String(s);
                }
                // The newline is left for the next token.
                Some('\n') => return Token::BadString,
                Some('\\') => {
                    self.pos += 1;
                    match self.peek(0) {
                        None => {}
                        // An escaped newline continues the string.
                        Some('\n') => self.pos += 1,
                        Some(_) => s.push(self.consume_escape()),
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn consume_numeric(&mut self) -> Token {
        let number = self.consume_number();
        if self.starts_ident_at(0) {
            Token::Dimension(number, self.consume_name())
        } else if self.peek(0) == Some('%') {
            self.pos += 1;
            Token::Percentage(number)
        } else {
            Token::Number(number)
        }
    }

    fn consume_number(&mut self) -> Number {
        let start = self.pos;
        let signed = matches!(self.peek(0), Some('+' | '-'));
        if signed {
            self.pos += 1;
        }
        let digits = |t: &mut Self| {
            while t.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                t.pos += 1;
            }
        };
        digits(self);
        let mut integer = true;
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            integer = false;
            self.pos += 1;
            digits(self);
        }
        if matches!(self.peek(0), Some('e' | 'E')) {
            let exp_digits = match self.peek(1) {
                Some('+' | '-') => 2,
                _ => 1,
            };
            if self.peek(exp_digits).is_some_and(|c| c.is_ascii_digit()) {
                integer = false;
                self.pos += exp_digits;
                digits(self);
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        Number {
            value: text.parse().unwrap_or(0.0),
            integer,
            signed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Token::*;
    use super::{Number as Num, Token, tokenize};

    fn ident(s: &str) -> Token {
        Ident(s.into())
    }

    fn int(value: f64, signed: bool) -> Num {
        Num {
            value,
            integer: true,
            signed,
        }
    }

    #[test]
    fn simple_selector_tokens() {
        assert_eq!(
            tokenize("div.a > #b[c='d'] :not(e)"),
            [
                ident("div"),
                Delim('.'),
                ident("a"),
                Whitespace,
                Delim('>'),
                Whitespace,
                Hash {
                    value: "b".into(),
                    id: true
                },
                LBracket,
                ident("c"),
                Delim('='),
                String("d".into()),
                RBracket,
                Whitespace,
                Colon,
                Function("not".into()),
                ident("e"),
                RParen,
            ]
        );
    }

    #[test]
    fn hashes_record_whether_they_are_identifiers() {
        assert_eq!(
            tokenize("#1a"),
            [Hash {
                value: "1a".into(),
                id: false
            }]
        );
        assert_eq!(
            tokenize("#-a"),
            [Hash {
                value: "-a".into(),
                id: true
            }]
        );
        assert_eq!(tokenize("# a"), [Delim('#'), Whitespace, ident("a")]);
    }

    #[test]
    fn escapes() {
        // Tailwind-style class names and escaped leading digits.
        assert_eq!(tokenize(r"md\:flex"), [ident("md:flex")]);
        assert_eq!(tokenize(r"w-1\/2"), [ident("w-1/2")]);
        assert_eq!(
            tokenize(r"#\31 23"),
            [Hash {
                value: "123".into(),
                id: true
            }]
        );
        assert_eq!(tokenize(r"\0"), [ident("\u{FFFD}")]);
        assert_eq!(tokenize(r"\110000"), [ident("\u{FFFD}")]);
        assert_eq!(tokenize(r"a\"), [ident("a"), Delim('\\')]);
        assert_eq!(tokenize("'a\\\nb\\'c'"), [String("ab'c".into())]);
    }

    #[test]
    fn strings() {
        assert_eq!(tokenize(r#""a'b""#), [String("a'b".into())]);
        assert_eq!(tokenize("'unclosed"), [String("unclosed".into())]);
        assert_eq!(
            tokenize("'a\nb'"),
            [BadString, Whitespace, ident("b"), String("".into())]
        );
    }

    #[test]
    fn numbers_and_dimensions() {
        assert_eq!(
            tokenize("2n+1"),
            [
                Dimension(int(2.0, false), "n".into()),
                Number(int(1.0, true))
            ]
        );
        assert_eq!(
            tokenize("-2n-1"),
            [Dimension(int(-2.0, true), "n-1".into())]
        );
        assert_eq!(tokenize("-n-1"), [ident("-n-1")]);
        assert_eq!(tokenize("+n"), [Delim('+'), ident("n")]);
        assert_eq!(
            tokenize("1.5"),
            [Number(Num {
                value: 1.5,
                integer: false,
                signed: false
            })]
        );
        assert_eq!(
            tokenize("1e3"),
            [Number(Num {
                value: 1000.0,
                integer: false,
                signed: false
            })]
        );
        assert_eq!(tokenize("5%"), [Percentage(int(5.0, false))]);
        assert_eq!(tokenize(".5").len(), 1);
        assert_eq!(tokenize(".a"), [Delim('.'), ident("a")]);
    }

    #[test]
    fn comments_and_whitespace() {
        assert_eq!(
            tokenize("a/* x */b /**/ \r\n\t c/* unclosed"),
            // A comment between two runs of whitespace separates them.
            [ident("a"), ident("b"), Whitespace, Whitespace, ident("c")]
        );
    }

    #[test]
    fn misc_tokens() {
        assert_eq!(
            tokenize("<!-- --> @media -- a|b"),
            [
                Cdo,
                Whitespace,
                Cdc,
                Whitespace,
                AtKeyword("media".into()),
                Whitespace,
                ident("--"),
                Whitespace,
                ident("a"),
                Delim('|'),
                ident("b"),
            ]
        );
        assert_eq!(tokenize("\0"), [ident("\u{FFFD}")]);
    }
}
