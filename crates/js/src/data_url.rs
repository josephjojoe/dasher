//! `data:` URLs, which `fetch()` and `XMLHttpRequest` answer without going
//! to the network.
//!
//! <https://fetch.spec.whatwg.org/#data-url-processor>

/// The decoded contents of a `data:` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataUrl {
    /// The MIME type, as written (or `text/plain;charset=US-ASCII`).
    pub(crate) mime_type: String,
    pub(crate) body: Vec<u8>,
}

/// Decodes the `data:` URL `url` into its MIME type and body, for hosts
/// that load classic scripts from one. `None` if it is not a valid one.
pub fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    parse(url).map(|data| (data.mime_type, data.body))
}

/// Decodes `url`, the full serialized URL including the `data:` scheme.
/// Returns `None` if it is not a valid `data:` URL.
pub(crate) fn parse(url: &str) -> Option<DataUrl> {
    let rest = url
        .get(..5)
        .filter(|scheme| scheme.eq_ignore_ascii_case("data:"))
        .map(|_| &url[5..])?;
    // The fragment is not part of the data.
    let rest = rest.split('#').next().unwrap_or_default();
    let (mime, encoded) = rest.split_once(',')?;
    let mut mime = mime.trim_matches(is_ascii_whitespace);
    let mut body = percent_decode(encoded.as_bytes());

    if let Some(prefix) = strip_base64_suffix(mime) {
        body = forgiving_base64_decode(&body)?;
        mime = prefix.trim_end_matches(is_ascii_whitespace);
    }
    let mime_type = if mime.starts_with(';') {
        format!("text/plain{mime}")
    } else if mime.is_empty() {
        String::from("text/plain;charset=US-ASCII")
    } else {
        mime.to_owned()
    };
    Some(DataUrl { mime_type, body })
}

fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// `mime` without a trailing `;base64` parameter, if it has one.
fn strip_base64_suffix(mime: &str) -> Option<&str> {
    let (prefix, last) = mime.rsplit_once(';')?;
    last.trim_matches(is_ascii_whitespace)
        .eq_ignore_ascii_case("base64")
        .then_some(prefix)
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |b: u8| char::from(b).to_digit(16);
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let byte = input[i];
        if byte == b'%' && i + 2 < input.len() {
            if let (Some(hi), Some(lo)) = (hex(input[i + 1]), hex(input[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(byte);
        i += 1;
    }
    out
}

/// <https://infra.spec.whatwg.org/#forgiving-base64-decode>
pub(crate) fn forgiving_base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' '))
        .collect();
    if data.len() % 4 == 0 {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let value = |b: u8| -> Option<u32> {
        Some(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut n = 0u32;
        for &b in chunk {
            n = n << 6 | value(b)?;
        }
        // Left-align a short final chunk as if it were padded with zeros.
        n <<= 6 * (4 - chunk.len() as u32);
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(url: &str) -> Option<(String, Vec<u8>)> {
        parse(url).map(|d| (d.mime_type, d.body))
    }

    #[test]
    fn plain_and_percent_encoded() {
        assert_eq!(
            data("data:,Hello%2C%20World!"),
            Some((
                "text/plain;charset=US-ASCII".into(),
                b"Hello, World!".to_vec()
            ))
        );
        assert_eq!(
            data("data:application/json,%7B%22a%22%3A1%7D"),
            Some(("application/json".into(), br#"{"a":1}"#.to_vec()))
        );
        assert_eq!(
            data("DATA:;charset=utf-8,x"),
            Some(("text/plain;charset=utf-8".into(), b"x".to_vec()))
        );
        // Bad escapes pass through.
        assert_eq!(
            data("data:,100%"),
            Some(("text/plain;charset=US-ASCII".into(), b"100%".to_vec()))
        );
        assert_eq!(
            data("data:,a%zzb%4"),
            Some(("text/plain;charset=US-ASCII".into(), b"a%zzb%4".to_vec()))
        );
    }

    #[test]
    fn base64() {
        assert_eq!(
            data("data:text/plain;base64,SGVsbG8="),
            Some(("text/plain".into(), b"Hello".to_vec()))
        );
        assert_eq!(
            data("data:application/octet-stream ; BASE64 ,SGVs bG8#frag"),
            Some(("application/octet-stream".into(), b"Hello".to_vec()))
        );
        assert_eq!(data("data:;base64,SGVsbG8").unwrap().1, b"Hello");
        assert_eq!(data("data:;base64,%53GVsbG8=").unwrap().1, b"Hello");
        assert_eq!(data("data:;base64,S"), None);
        assert_eq!(data("data:;base64,SG!s"), None);
    }

    #[test]
    fn invalid() {
        assert_eq!(data("data:text/plain"), None);
        assert_eq!(data("http://a.com/,x"), None);
        assert_eq!(data("dat"), None);
    }

    #[test]
    fn forgiving_base64() {
        let d = |s: &str| forgiving_base64_decode(s.as_bytes());
        assert_eq!(d(""), Some(vec![]));
        assert_eq!(d("Zg=="), Some(b"f".to_vec()));
        assert_eq!(d("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(d("Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(d("Zm9vYg"), Some(b"foob".to_vec()));
        assert_eq!(d(" Zm9v\nYmFy "), Some(b"foobar".to_vec()));
        assert_eq!(d("Zg="), None);
        assert_eq!(d("Z"), None);
        assert_eq!(d("Zm9v="), None);
    }
}
