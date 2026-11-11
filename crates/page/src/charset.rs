//! Working out a document's character encoding and decoding it.
//!
//! The order follows the HTML standard's encoding sniffing algorithm: a
//! byte order mark, then the `charset` parameter of `Content-Type`, then a
//! `<meta charset>` or `<meta http-equiv=content-type>` found by prescanning
//! the first bytes. With none of those, browsers guess from the user's
//! locale (windows-1252 for English). dasher guesses UTF-8 unless the bytes
//! it has seen are not valid UTF-8, since that is what nearly every page
//! without a declaration is today.
//! <https://html.spec.whatwg.org/#encoding-sniffing-algorithm>

use encoding_rs::{Decoder, Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED};

/// How many bytes are prescanned for a `<meta>` declaration.
pub(crate) const PRESCAN_LEN: usize = 1024;

/// The encoding named by the `charset` parameter of a `Content-Type` value.
pub(crate) fn from_content_type(content_type: &str) -> Option<&'static Encoding> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.split('"').next())
            .unwrap_or(value);
        Encoding::for_label(value.as_bytes())
    })
}

/// The encoding for a document whose first bytes are `head`, served with
/// `content_type`. `complete` says whether `head` is the whole document;
/// otherwise it should hold at least [`PRESCAN_LEN`] bytes.
pub(crate) fn sniff(content_type: Option<&str>, head: &[u8], complete: bool) -> &'static Encoding {
    if let Some((encoding, _)) = Encoding::for_bom(head) {
        return encoding;
    }
    if let Some(encoding) = content_type.and_then(from_content_type) {
        return encoding;
    }
    if let Some(encoding) = prescan(&head[..head.len().min(PRESCAN_LEN)]) {
        return encoding;
    }
    if looks_like_utf8(head, complete) {
        UTF_8
    } else {
        WINDOWS_1252
    }
}

/// Whether `bytes` are valid UTF-8, allowing a sequence cut off at the end
/// when more bytes are to come.
fn looks_like_utf8(bytes: &[u8], complete: bool) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => !complete && e.error_len().is_none(),
    }
}

/// A decoder for `encoding` that drops a leading byte order mark.
pub(crate) fn decoder(encoding: &'static Encoding) -> Decoder {
    encoding.new_decoder_with_bom_removal()
}

/// Decodes the next chunk of a stream. `last` marks the end of the input.
pub(crate) fn decode_chunk(decoder: &mut Decoder, bytes: &[u8], last: bool) -> String {
    let capacity = decoder
        .max_utf8_buffer_length(bytes.len())
        .unwrap_or(bytes.len().saturating_mul(3));
    let mut out = String::with_capacity(capacity);
    let (_, read, _) = decoder.decode_to_string(bytes, &mut out, last);
    debug_assert_eq!(read, bytes.len(), "the buffer is large enough");
    out.shrink_to_fit();
    out
}

/// Decodes a classic script: a byte order mark wins, then `fallback` (the
/// response's charset, the element's `charset` attribute, or the
/// document's encoding).
pub(crate) fn decode_script(bytes: &[u8], fallback: &'static Encoding) -> String {
    fallback.decode(bytes).0.into_owned()
}

// ---------------------------------------------------------------------------
// The prescan.
// https://html.spec.whatwg.org/#prescan-a-byte-stream-to-determine-its-encoding

fn is_space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

/// Finds the encoding a `<meta>` near the start of `bytes` declares.
pub(crate) fn prescan(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut pos = 0;
    while pos < bytes.len() {
        let rest = &bytes[pos..];
        if rest.starts_with(b"<!--") {
            // The dashes of `<!--` may also end it, as in `<!-->`.
            let end = find(&bytes[pos + 2..], b"-->")?;
            pos += 2 + end + 3;
            continue;
        }
        if rest.len() > 5
            && rest[..5].eq_ignore_ascii_case(b"<meta")
            && (is_space(rest[5]) || rest[5] == b'/')
        {
            pos += 6;
            if let Some(encoding) = meta(bytes, &mut pos)? {
                return Some(encoding);
            }
            continue;
        }
        let tag_start = match rest {
            [b'<', b'/', c, ..] | [b'<', c, ..] => c.is_ascii_alphabetic(),
            _ => false,
        };
        if tag_start {
            // Skip the tag name, then its attributes.
            while pos < bytes.len() && !is_space(bytes[pos]) && bytes[pos] != b'>' {
                pos += 1;
            }
            while attribute(bytes, &mut pos)?.is_some() {}
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") || rest.starts_with(b"<?") {
            pos += find(rest, b">")?;
        }
        pos += 1;
    }
    None
}

/// Reads the attributes of a `<meta>` element. Returns the encoding it
/// declares, `Some(None)` if it declares none, or `None` if the input ran
/// out.
fn meta(bytes: &[u8], pos: &mut usize) -> Option<Option<&'static Encoding>> {
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let mut got_pragma = false;
    let mut need_pragma = None;
    let mut charset = None;
    while let Some((name, value)) = attribute(bytes, pos)? {
        if seen.contains(&name) {
            continue;
        }
        match name.as_slice() {
            b"http-equiv" => got_pragma |= value == b"content-type",
            b"content" if charset.is_none() => {
                if let Some(label) = charset_from_content(&value) {
                    charset = Encoding::for_label(label);
                    need_pragma = Some(true);
                }
            }
            b"charset" => {
                charset = Encoding::for_label(&value);
                need_pragma = Some(false);
            }
            _ => {}
        }
        seen.push(name);
    }
    let declared = match need_pragma {
        None => None,
        Some(true) if !got_pragma => None,
        _ => charset.map(|c| {
            if c == UTF_16BE || c == UTF_16LE {
                UTF_8
            } else if c == X_USER_DEFINED {
                WINDOWS_1252
            } else {
                c
            }
        }),
    };
    Some(declared)
}

/// The "get an attribute" step: reads one attribute at `pos`, lowercasing
/// ASCII. Returns `Some(None)` at the end of the tag and `None` if the input
/// ran out.
#[allow(clippy::type_complexity)]
fn attribute(bytes: &[u8], pos: &mut usize) -> Option<Option<(Vec<u8>, Vec<u8>)>> {
    let at = |p: usize| bytes.get(p).copied();
    while at(*pos).is_some_and(|b| is_space(b) || b == b'/') {
        *pos += 1;
    }
    if at(*pos)? == b'>' {
        return Some(None);
    }
    let mut name = Vec::new();
    let mut value = Vec::new();
    // The name.
    loop {
        let b = at(*pos)?;
        if b == b'=' && !name.is_empty() {
            *pos += 1;
            break;
        }
        if is_space(b) {
            while at(*pos).is_some_and(is_space) {
                *pos += 1;
            }
            if at(*pos)? != b'=' {
                return Some(Some((name, value)));
            }
            *pos += 1;
            break;
        }
        if b == b'/' || b == b'>' {
            return Some(Some((name, value)));
        }
        name.push(b.to_ascii_lowercase());
        *pos += 1;
    }
    // The value.
    while at(*pos).is_some_and(is_space) {
        *pos += 1;
    }
    let b = at(*pos)?;
    if b == b'"' || b == b'\'' {
        loop {
            *pos += 1;
            let c = at(*pos)?;
            if c == b {
                *pos += 1;
                return Some(Some((name, value)));
            }
            value.push(c.to_ascii_lowercase());
        }
    }
    if b == b'>' {
        return Some(Some((name, value)));
    }
    loop {
        let c = at(*pos)?;
        if is_space(c) || c == b'>' {
            return Some(Some((name, value)));
        }
        value.push(c.to_ascii_lowercase());
        *pos += 1;
    }
}

/// Extracts the charset label from a `<meta content>` value such as
/// `text/html; charset=utf-8` (already lowercased).
/// <https://html.spec.whatwg.org/#algorithm-for-extracting-a-character-encoding-from-a-meta-element>
fn charset_from_content(content: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    loop {
        pos += find(&content[pos..], b"charset")? + 7;
        let mut p = pos;
        while content.get(p).copied().is_some_and(is_space) {
            p += 1;
        }
        if content.get(p) != Some(&b'=') {
            continue;
        }
        p += 1;
        while content.get(p).copied().is_some_and(is_space) {
            p += 1;
        }
        let rest = &content[p..];
        return match rest.first()? {
            &q @ (b'"' | b'\'') => {
                let end = rest[1..].iter().position(|&c| c == q)?;
                Some(&rest[1..1 + end])
            }
            _ => {
                let end = rest
                    .iter()
                    .position(|&c| is_space(c) || c == b';')
                    .unwrap_or(rest.len());
                Some(&rest[..end])
            }
        };
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use encoding_rs::{EUC_JP, GBK, ISO_8859_2, SHIFT_JIS, WINDOWS_1251};

    use super::*;

    #[test]
    fn content_type_charset() {
        assert_eq!(from_content_type("text/html; charset=utf-8"), Some(UTF_8));
        assert_eq!(
            from_content_type("text/html;CHARSET=\"Shift_JIS\"; foo=bar"),
            Some(SHIFT_JIS)
        );
        assert_eq!(
            from_content_type("text/html; charset = latin1"),
            Some(WINDOWS_1252)
        );
        assert_eq!(from_content_type("text/html"), None);
        assert_eq!(from_content_type("text/html; charset=bogus"), None);
    }

    #[test]
    fn meta_declarations() {
        let cases: &[(&[u8], Option<&'static Encoding>)] = &[
            (b"<meta charset=gbk>", Some(GBK)),
            (b"<META CHARSET='Windows-1251'>", Some(WINDOWS_1251)),
            (
                b"<!doctype html><html><head><meta charset=\"euc-jp\">",
                Some(EUC_JP),
            ),
            (
                b"<meta http-equiv=Content-Type content='text/html; charset=iso-8859-2'>",
                Some(ISO_8859_2),
            ),
            (
                b"<meta content=\"text/html;charset = 'shift_jis'\" http-equiv=\"content-type\">",
                Some(SHIFT_JIS),
            ),
            // `content` needs the pragma.
            (b"<meta content='text/html; charset=gbk'>", None),
            // The first value of a repeated attribute counts.
            (b"<meta charset=gbk charset=utf-8>", Some(GBK)),
            // UTF-16 in a meta means UTF-8; x-user-defined means windows-1252.
            (b"<meta charset=utf-16le>", Some(UTF_8)),
            (b"<meta charset=x-user-defined>", Some(WINDOWS_1252)),
            // Comments, other tags and their attributes are skipped.
            (
                b"<!-- <meta charset=gbk> --><meta charset=euc-jp>",
                Some(EUC_JP),
            ),
            (b"<!--><meta charset=gbk>", Some(GBK)),
            (
                b"<div title='<meta charset=gbk>'><meta charset=euc-jp>",
                Some(EUC_JP),
            ),
            (b"<metadata charset=gbk>", None),
            (b"<meta name=viewport><meta charset=gbk>", Some(GBK)),
            (b"<meta charset=bogus><meta charset=gbk>", Some(GBK)),
            // Cut off mid-tag.
            (b"<meta charset='gbk", None),
            (b"<p>no declaration</p>", None),
        ];
        for &(html, expected) in cases {
            assert_eq!(
                prescan(html),
                expected,
                "{:?}",
                String::from_utf8_lossy(html)
            );
        }
    }

    #[test]
    fn sniffing_order() {
        let meta = b"<meta charset=gbk>";
        // A byte order mark beats everything.
        let mut bom = b"\xEF\xBB\xBF".to_vec();
        bom.extend_from_slice(meta);
        assert_eq!(
            sniff(Some("text/html; charset=shift_jis"), &bom, true),
            UTF_8
        );
        assert_eq!(sniff(None, b"\xFF\xFEa\0", true), UTF_16LE);
        // Then the header, then the meta.
        assert_eq!(
            sniff(Some("text/html; charset=shift_jis"), meta, true),
            SHIFT_JIS
        );
        assert_eq!(sniff(Some("text/html"), meta, true), GBK);
        // Then a guess.
        assert_eq!(sniff(None, "café".as_bytes(), true), UTF_8);
        assert_eq!(sniff(None, b"caf\xE9", true), WINDOWS_1252);
        assert_eq!(sniff(None, b"caf\xC3", false), UTF_8);
        assert_eq!(sniff(None, b"caf\xC3", true), WINDOWS_1252);
    }

    #[test]
    fn streaming_decode_handles_split_sequences() {
        let mut d = decoder(UTF_8);
        let bytes = "\u{feff}héllo ☺".as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(1) {
            out.push_str(&decode_chunk(&mut d, chunk, false));
        }
        out.push_str(&decode_chunk(&mut d, &[], true));
        assert_eq!(out, "héllo ☺");

        let mut d = decoder(WINDOWS_1252);
        assert_eq!(decode_chunk(&mut d, b"caf\xE9 \x80", true), "café €");
    }

    #[test]
    fn scripts_decode_with_bom_sniffing() {
        assert_eq!(
            decode_script(b"var s = '\xE9';", WINDOWS_1252),
            "var s = 'é';"
        );
        assert_eq!(
            decode_script("\u{feff}var s = 'é';".as_bytes(), WINDOWS_1252),
            "var s = 'é';"
        );
    }
}
