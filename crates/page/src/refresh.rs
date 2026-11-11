//! Declarative refreshes: `<meta http-equiv="refresh">` and the `Refresh`
//! response header.
//!
//! <https://html.spec.whatwg.org/#shared-declarative-refresh-steps>

use std::time::Duration;

use dasher_dom::{Document, Name};
use dasher_net::Url;

/// A refresh a document asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refresh {
    /// How long after the document has loaded.
    pub(crate) delay: Duration,
    /// Where to go: the document's own URL when the refresh named none.
    pub(crate) url: Url,
}

impl Refresh {
    /// Parses a refresh value such as `5; url=/next`, resolving its URL
    /// against `base` and defaulting to `document_url`. `None` if the value
    /// is not a valid refresh, which browsers ignore.
    pub(crate) fn parse(value: &str, base: &Url, document_url: &Url) -> Option<Refresh> {
        let input = value.as_bytes();
        let mut pos = skip_whitespace(input, 0);

        // The delay in whole seconds. Fractions are allowed and ignored.
        let digits_end = pos
            + input[pos..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
        let digits = &value[pos..digits_end];
        if digits.is_empty() && input.get(pos) != Some(&b'.') {
            return None;
        }
        // Absurdly long delays saturate rather than fail.
        let seconds = digits
            .parse::<u64>()
            .unwrap_or(if digits.is_empty() { 0 } else { u64::MAX });
        pos = digits_end;
        pos += input[pos..]
            .iter()
            .take_while(|&&b| b.is_ascii_digit() || b == b'.')
            .count();
        let delay = Duration::from_secs(seconds);

        if pos == input.len() {
            return Some(Refresh {
                delay,
                url: document_url.clone(),
            });
        }
        if !matches!(input[pos], b';' | b',') && !is_whitespace(input[pos]) {
            return None;
        }
        pos = skip_whitespace(input, pos);
        if matches!(input.get(pos), Some(b';' | b',')) {
            pos += 1;
        }
        pos = skip_whitespace(input, pos);
        if pos == input.len() {
            return Some(Refresh {
                delay,
                url: document_url.clone(),
            });
        }

        // An optional `url=` prefix, then an optionally quoted URL.
        let rest = &value[pos..];
        let url_start = url_prefix_len(rest).unwrap_or(0);
        let mut url = &rest[url_start..];
        if let Some(quote @ ('\'' | '"')) = url.chars().next() {
            url = &url[1..];
            if let Some(end) = url.find(quote) {
                url = &url[..end];
            }
        }
        let url = base.join(url).ok()?;
        Some(Refresh { delay, url })
    }

    /// Whether the refresh reloads the document it is in, as pages that
    /// poll for updates do, rather than going elsewhere.
    pub(crate) fn reloads(&self, document_url: &Url) -> bool {
        let mut url = self.url.clone();
        let mut current = document_url.clone();
        url.set_fragment(None);
        current.set_fragment(None);
        url == current
    }
}

/// How long a `url=` prefix at the start of `s` is, with the whitespace
/// around its `=`, if there is one.
fn url_prefix_len(s: &str) -> Option<usize> {
    let input = s.as_bytes();
    if !input.get(..3)?.eq_ignore_ascii_case(b"url") {
        return None;
    }
    let pos = skip_whitespace(input, 3);
    if input.get(pos) != Some(&b'=') {
        return None;
    }
    Some(skip_whitespace(input, pos + 1))
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn skip_whitespace(input: &[u8], pos: usize) -> usize {
    pos + input[pos..]
        .iter()
        .take_while(|&&b| is_whitespace(b))
        .count()
}

/// The first valid refresh among the document's
/// `<meta http-equiv="refresh">` elements, in tree order. Elements in
/// `<template>` contents are not in the tree and don't count, nor does
/// `<noscript>` content, which is text with scripting on.
pub(crate) fn find_meta(doc: &Document, base: &Url, document_url: &Url) -> Option<Refresh> {
    doc.descendants(doc.root())
        .filter(|&n| {
            doc.is_html(n, Name::META)
                && doc
                    .attribute(n, "http-equiv")
                    .is_some_and(|v| v.trim_matches(is_space).eq_ignore_ascii_case("refresh"))
        })
        .filter_map(|n| doc.attribute(n, "content"))
        .find_map(|content| Refresh::parse(content, base, document_url))
}

fn is_space(c: char) -> bool {
    c.is_ascii() && is_whitespace(c as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> Option<(u64, String)> {
        let document = Url::parse("https://example.com/dir/page?q=1#top").unwrap();
        let base = Url::parse("https://example.com/base/").unwrap();
        Refresh::parse(value, &base, &document).map(|r| (r.delay.as_secs(), r.url.to_string()))
    }

    fn some(seconds: u64, url: &str) -> Option<(u64, String)> {
        Some((seconds, url.to_owned()))
    }

    #[test]
    fn parses_delays_and_urls() {
        let here = "https://example.com/dir/page?q=1#top";
        assert_eq!(parse("0; url=/next"), some(0, "https://example.com/next"));
        assert_eq!(
            parse("5;URL=next"),
            some(5, "https://example.com/base/next")
        );
        assert_eq!(
            parse(" 3 , url = 'a b' tail"),
            some(3, "https://example.com/base/a%20b")
        );
        assert_eq!(parse("1; url=\"/q\""), some(1, "https://example.com/q"));
        assert_eq!(parse("1; url=\"/q"), some(1, "https://example.com/q"));
        assert_eq!(
            parse("2 https://other.test/x"),
            some(2, "https://other.test/x")
        );
        assert_eq!(parse("0;/next"), some(0, "https://example.com/next"));
        assert_eq!(
            parse("0; urlish"),
            some(0, "https://example.com/base/urlish")
        );
        assert_eq!(parse("1.5; url=/x"), some(1, "https://example.com/x"));
        assert_eq!(parse(".5; url=/x"), some(0, "https://example.com/x"));
        assert_eq!(parse("30"), some(30, here));
        assert_eq!(parse("30;  "), some(30, here));
        assert_eq!(
            parse("99999999999999999999999; url=/x"),
            some(u64::MAX, "https://example.com/x")
        );
    }

    #[test]
    fn rejects_invalid_values() {
        for value in [
            "",
            "  ",
            "url=/next",
            "-1; url=/x",
            "5x; url=/x",
            "; url=/x",
        ] {
            assert_eq!(parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn reloads_ignores_the_fragment() {
        let document = Url::parse("https://example.com/a#one").unwrap();
        let refresh = |url: &str| Refresh {
            delay: Duration::ZERO,
            url: Url::parse(url).unwrap(),
        };
        assert!(refresh("https://example.com/a").reloads(&document));
        assert!(refresh("https://example.com/a#two").reloads(&document));
        assert!(!refresh("https://example.com/b").reloads(&document));
        assert!(!refresh("https://example.com/a?x").reloads(&document));
    }

    #[test]
    fn finds_the_first_valid_meta() {
        let url = Url::parse("https://example.com/").unwrap();
        let find = |html: &str| {
            let doc = Document::parse(html);
            find_meta(&doc, &url, &url).map(|r| (r.delay.as_secs(), r.url.to_string()))
        };
        assert_eq!(
            find(
                r#"<meta http-equiv="Content-Type" content="text/html">
                <meta http-equiv=refresh content="nonsense">
                <meta http-equiv=" REFRESH " content="2; url=/b">
                <meta http-equiv=refresh content="0; url=/c">"#
            ),
            some(2, "https://example.com/b")
        );
        assert_eq!(
            find(r#"<body><meta http-equiv=refresh content="0; url=/late">"#),
            some(0, "https://example.com/late")
        );
        assert_eq!(
            find(
                r#"<template><meta http-equiv=refresh content="0; url=/t"></template>
                <noscript><meta http-equiv=refresh content="0; url=/nojs"></noscript>
                <meta name=refresh content="0; url=/name">"#
            ),
            None
        );
    }
}
