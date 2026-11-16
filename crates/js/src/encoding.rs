//! Text encodings and base64 for `TextEncoder`, `TextDecoder`, `atob` and
//! `btoa`.
//!
//! Only the encodings scraping actually meets in script-visible decoders are
//! supported: UTF-8, UTF-16LE/BE and windows-1252 (which the Encoding
//! Standard also uses for `latin1`, `ascii` and `iso-8859-1`). Other labels
//! make the `TextDecoder` constructor throw a `RangeError`, as browsers do
//! for labels they do not know.

use rquickjs::{Array, Ctx, Function, Object, TypedArray, Value};

/// An encoding `TextDecoder` can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    /// Looks up an encoding label. https://encoding.spec.whatwg.org/#names-and-labels
    fn for_label(label: &str) -> Option<Self> {
        let label = label
            .trim_matches(|c| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '))
            .to_ascii_lowercase();
        Some(match label.as_str() {
            "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8" | "utf-8" | "utf8"
            | "x-unicode20utf8" => Self::Utf8,
            "csunicode" | "iso-10646-ucs-2" | "ucs-2" | "unicode" | "unicodefeff" | "utf-16"
            | "utf-16le" => Self::Utf16Le,
            "unicodefffe" | "utf-16be" => Self::Utf16Be,
            "ansi_x3.4-1968" | "ascii" | "cp1252" | "cp819" | "csisolatin1" | "ibm819"
            | "iso-8859-1" | "iso-ir-100" | "iso8859-1" | "iso88591" | "iso_8859-1"
            | "iso_8859-1:1987" | "l1" | "latin1" | "us-ascii" | "windows-1252" | "x-cp1252" => {
                Self::Windows1252
            }
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
            Self::Windows1252 => "windows-1252",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::Utf8, Self::Utf16Le, Self::Utf16Be, Self::Windows1252]
            .into_iter()
            .find(|e| e.name() == name)
    }
}

/// The result of decoding one chunk.
#[derive(Debug, PartialEq, Eq)]
struct Decoded {
    text: String,
    /// Bytes consumed. In streaming mode an incomplete sequence at the end is
    /// left for the next chunk.
    consumed: usize,
}

/// Decodes `bytes`. Returns `None` when `fatal` is set and the input is
/// malformed; otherwise malformed sequences become U+FFFD.
fn decode(encoding: Encoding, bytes: &[u8], fatal: bool, stream: bool) -> Option<Decoded> {
    match encoding {
        Encoding::Utf8 => decode_utf8(bytes, fatal, stream),
        Encoding::Utf16Le => decode_utf16(bytes, fatal, stream, u16::from_le_bytes),
        Encoding::Utf16Be => decode_utf16(bytes, fatal, stream, u16::from_be_bytes),
        Encoding::Windows1252 => Some(Decoded {
            text: bytes.iter().map(|&b| windows_1252(b)).collect(),
            consumed: bytes.len(),
        }),
    }
}

fn decode_utf8(bytes: &[u8], fatal: bool, stream: bool) -> Option<Decoded> {
    let end = if stream {
        incomplete_utf8_tail(bytes)
    } else {
        bytes.len()
    };
    let input = &bytes[..end];
    let text = match std::str::from_utf8(input) {
        Ok(text) => text.to_owned(),
        Err(_) if fatal => return None,
        // `from_utf8_lossy` replaces maximal subparts, as the Encoding
        // Standard's UTF-8 decoder does.
        Err(_) => String::from_utf8_lossy(input).into_owned(),
    };
    Some(Decoded {
        text,
        consumed: end,
    })
}

/// Where a trailing, possibly-valid but incomplete UTF-8 sequence starts, or
/// `bytes.len()` if there is none.
fn incomplete_utf8_tail(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for start in (len.saturating_sub(3)..len).rev() {
        if bytes[start] & 0xC0 != 0x80 {
            return match std::str::from_utf8(&bytes[start..]) {
                Err(e) if e.valid_up_to() == 0 && e.error_len().is_none() => start,
                _ => len,
            };
        }
    }
    len
}

fn decode_utf16(
    bytes: &[u8],
    fatal: bool,
    stream: bool,
    unit: fn([u8; 2]) -> u16,
) -> Option<Decoded> {
    let mut units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| unit([pair[0], pair[1]]))
        .collect();
    let mut consumed = units.len() * 2;
    let odd_byte = bytes.len() % 2 == 1;
    if stream {
        if units.last().is_some_and(|&u| (0xD800..0xDC00).contains(&u)) {
            units.pop();
            consumed -= 2;
        }
    } else {
        consumed = bytes.len();
    }
    let mut text = String::with_capacity(units.len());
    for c in char::decode_utf16(units) {
        match c {
            Ok(c) => text.push(c),
            Err(_) if fatal => return None,
            Err(_) => text.push(char::REPLACEMENT_CHARACTER),
        }
    }
    if odd_byte && !stream {
        if fatal {
            return None;
        }
        text.push(char::REPLACEMENT_CHARACTER);
    }
    Some(Decoded { text, consumed })
}

/// windows-1252 bytes 0x80 to 0x9F; the rest map to the same code point.
const WINDOWS_1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
];

fn windows_1252(byte: u8) -> char {
    match byte {
        0x80..=0x9F => WINDOWS_1252_HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// UTF-8 encodes as much of `text` as fits in `max` bytes without splitting
/// a character. Returns the byte length and how many UTF-16 code units of
/// `text` that covers.
fn utf8_prefix(text: &str, max: usize) -> (usize, usize) {
    let mut bytes = 0;
    let mut units = 0;
    for c in text.chars() {
        if bytes + c.len_utf8() > max {
            break;
        }
        bytes += c.len_utf8();
        units += c.len_utf16();
    }
    (bytes, units)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64-encodes `bytes` with padding.
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(BASE64[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The forgiving-base64 decode. Returns `None` on failure.
/// https://infra.spec.whatwg.org/#forgiving-base64-decode
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .bytes()
        .filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' '))
        .collect();
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in &data {
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = buffer << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Adds the encoding host functions to the prelude's `native` object.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, native: &Object<'js>) -> rquickjs::Result<()> {
    native.set(
        "encodingName",
        Function::new(ctx.clone(), |label: String| {
            Encoding::for_label(&label).map(Encoding::name)
        })?,
    )?;
    native.set(
        "utf8Encode",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
            TypedArray::<u8>::new(ctx, text.into_bytes())
        })?,
    )?;
    native.set(
        "utf8EncodePrefix",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String, max: f64| -> rquickjs::Result<Array<'js>> {
                // `max` is a byte length, which always fits in usize.
                let max = if max > 0.0 { max as usize } else { 0 };
                let (bytes, units) = utf8_prefix(&text, max);
                let out = Array::new(ctx.clone())?;
                out.set(
                    0,
                    TypedArray::<u8>::new_copy(ctx, &text.as_bytes()[..bytes])?,
                )?;
                out.set(1, units)?;
                Ok(out)
            },
        )?,
    )?;
    native.set(
        "decode",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             name: String,
             bytes: TypedArray<'js, u8>,
             fatal: bool,
             stream: bool|
             -> rquickjs::Result<Value<'js>> {
                let encoding = Encoding::from_name(&name).unwrap_or(Encoding::Utf8);
                let input = bytes.as_bytes().unwrap_or_default();
                let Some(decoded) = decode(encoding, input, fatal, stream) else {
                    return Ok(Value::new_null(ctx));
                };
                let out = Array::new(ctx)?;
                out.set(0, decoded.text)?;
                out.set(1, decoded.consumed)?;
                Ok(out.into_value())
            },
        )?,
    )?;
    native.set(
        "btoa",
        Function::new(ctx.clone(), |latin1: String| {
            // The glue has already checked every code unit is at most 0xFF.
            let bytes: Vec<u8> = latin1.chars().map(|c| c as u32 as u8).collect();
            base64_encode(&bytes)
        })?,
    )?;
    native.set(
        "atob",
        Function::new(ctx.clone(), |data: String| {
            base64_decode(&data).map(|bytes| bytes.into_iter().map(char::from).collect::<String>())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(encoding: Encoding, bytes: &[u8]) -> String {
        decode(encoding, bytes, false, false).unwrap().text
    }

    #[test]
    fn labels() {
        assert_eq!(Encoding::for_label(" UTF8\n"), Some(Encoding::Utf8));
        assert_eq!(Encoding::for_label("utf-16"), Some(Encoding::Utf16Le));
        assert_eq!(Encoding::for_label("latin1"), Some(Encoding::Windows1252));
        assert_eq!(Encoding::for_label("shift_jis"), None);
    }

    #[test]
    fn utf8_replacement_and_fatal() {
        assert_eq!(text(Encoding::Utf8, b"a\xFFb"), "a\u{FFFD}b");
        // Maximal subparts: a truncated 3-byte sequence is one U+FFFD.
        assert_eq!(text(Encoding::Utf8, b"\xE2\x82z"), "\u{FFFD}z");
        assert_eq!(decode(Encoding::Utf8, b"a\xFF", true, false), None);
    }

    #[test]
    fn utf8_streaming_keeps_incomplete_tails() {
        let euro = "€".as_bytes();
        let first = decode(Encoding::Utf8, &[b'a', euro[0], euro[1]], false, true).unwrap();
        assert_eq!(first.text, "a");
        assert_eq!(first.consumed, 1);
        // An invalid tail is not held back.
        let bad = decode(Encoding::Utf8, b"a\xE2z", false, true).unwrap();
        assert_eq!((bad.text.as_str(), bad.consumed), ("a\u{FFFD}z", 3));
        assert_eq!(incomplete_utf8_tail(b"\xF0\x9F\x98"), 0);
        assert_eq!(incomplete_utf8_tail(b"ok"), 2);
    }

    #[test]
    fn utf16() {
        assert_eq!(text(Encoding::Utf16Le, b"h\0i\0"), "hi");
        assert_eq!(text(Encoding::Utf16Be, b"\0h\0i"), "hi");
        assert_eq!(text(Encoding::Utf16Le, b"\x3D\xD8\x00\xDE"), "😀");
        assert_eq!(text(Encoding::Utf16Le, b"\x3D\xD8h\0"), "\u{FFFD}h");
        assert_eq!(text(Encoding::Utf16Le, b"h\0i"), "h\u{FFFD}");
        assert_eq!(decode(Encoding::Utf16Le, b"h\0i", true, false), None);
        let held = decode(Encoding::Utf16Le, b"h\0\x3D\xD8\x00", false, true).unwrap();
        assert_eq!((held.text.as_str(), held.consumed), ("h", 2));
    }

    #[test]
    fn windows_1252_maps_the_c1_range() {
        assert_eq!(
            text(Encoding::Windows1252, b"caf\xE9 \x80\x99"),
            "café €\u{2122}"
        );
    }

    #[test]
    fn utf8_prefix_never_splits_characters() {
        assert_eq!(utf8_prefix("a€😀", 3), (1, 1));
        assert_eq!(utf8_prefix("a€😀", 4), (4, 2));
        assert_eq!(utf8_prefix("a€😀", 8), (8, 4));
    }

    #[test]
    fn base64_round_trips() {
        for (raw, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"\xFF\x00\x80", "/wCA"),
        ] {
            assert_eq!(base64_encode(raw), encoded);
            assert_eq!(base64_decode(encoded).as_deref(), Some(raw));
        }
    }

    #[test]
    fn base64_decode_is_forgiving_but_strict_about_shape() {
        assert_eq!(base64_decode(" Zm 9v\n").as_deref(), Some(&b"foo"[..]));
        assert_eq!(base64_decode("Zm8").as_deref(), Some(&b"fo"[..]));
        assert_eq!(base64_decode("Zg"), Some(b"f".to_vec()));
        assert_eq!(base64_decode("Z"), None);
        assert_eq!(base64_decode("Zg="), None);
        assert_eq!(base64_decode("Z=g="), None);
        assert_eq!(base64_decode("Zm9v!"), None);
    }
}
