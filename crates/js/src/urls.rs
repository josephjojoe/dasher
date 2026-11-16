//! WHATWG URL parsing for `URL` and `URLSearchParams`, via the `url` crate.
//!
//! A `URL` holds a [`UrlRecord`], the parsed [`Url`] itself: getters read
//! one component off it and setters change it in place, so nothing is
//! parsed twice and only the components a page reads become JS strings.
//! The last base URL parsed is kept per thread, since pages resolve many
//! URLs against the same base. `location` and the DOM's URL attributes go
//! through `urlParse`, which returns every component at once, and
//! `urlHref`, which returns only the serialization.

use std::cell::RefCell;

use rquickjs::class::{JsClass, Readable, Trace, Tracer};
use rquickjs::function::Constructor;
use rquickjs::{Class, Ctx, Function, JsLifetime, Object, Result, Value, qjs};
use url::{Url, form_urlencoded, quirks};

/// A URL's components in the order the glue reads them: `href`, `origin`,
/// `protocol`, `username`, `password`, `host`, `hostname`, `port`,
/// `pathname`, `search`, `hash`.
pub(crate) fn parts(url: &Url) -> Vec<String> {
    (0..PARTS).map(|i| part(url, i)).collect()
}

const PARTS: u32 = 11;

/// Component number `i` (see [`parts`]).
fn part(url: &Url, i: u32) -> String {
    match i {
        0 => quirks::href(url),
        1 => return quirks::origin(url),
        2 => quirks::protocol(url),
        3 => quirks::username(url),
        4 => quirks::password(url),
        5 => quirks::host(url),
        6 => quirks::hostname(url),
        7 => quirks::port(url),
        8 => quirks::pathname(url),
        9 => quirks::search(url),
        _ => quirks::hash(url),
    }
    .to_owned()
}

thread_local! {
    /// The last base URL parsed, as given, and what it parsed to.
    static BASE: RefCell<Option<(String, Option<Url>)>> = const { RefCell::new(None) };
}

/// Parses `input` against an optional `base`, as `new URL(input, base)`.
pub(crate) fn parse(input: &str, base: Option<&str>) -> Option<Url> {
    let Some(base) = base else {
        return Url::parse(input).ok();
    };
    BASE.with(|cell| {
        let mut cached = cell.borrow_mut();
        if cached.as_ref().is_none_or(|(text, _)| text != base) {
            *cached = Some((base.to_owned(), Url::parse(base).ok()));
        }
        cached.as_ref()?.1.as_ref()?.join(input).ok()
    })
}

/// Applies the `URL` attribute setter `part`. Returns false only when the
/// `href` setter fails, which throws; other setters ignore bad input.
fn set(url: &mut Url, part: &str, value: &str) -> bool {
    // The other setters leave the URL as it was when they fail.
    match part {
        "href" => return quirks::set_href(url, value).is_ok(),
        "protocol" => drop(quirks::set_protocol(url, value)),
        "username" => drop(quirks::set_username(url, value)),
        "password" => drop(quirks::set_password(url, value)),
        "host" => drop(quirks::set_host(url, value)),
        "hostname" => drop(quirks::set_hostname(url, value)),
        "port" => drop(quirks::set_port(url, value)),
        "pathname" => quirks::set_pathname(url, value),
        "search" => quirks::set_search(url, value),
        "hash" => quirks::set_hash(url, value),
        _ => {}
    }
    true
}

/// Runs `f` on a JS string as a USVString: lone surrogates become U+FFFD,
/// as `toWellFormed()` makes them. Valid strings are not copied.
fn with_usv<R>(ctx: &Ctx<'_>, value: &Value<'_>, f: impl FnOnce(&str) -> R) -> Result<R> {
    let mut len = 0;
    // SAFETY: `ctx` is live and locked for this call, and `value` belongs
    // to it. The C string QuickJS returns is valid for `len` bytes until it
    // is freed, exactly once, below, after the last use of `bytes`.
    unsafe {
        let ptr = qjs::JS_ToCStringLen(ctx.as_raw().as_ptr(), &mut len, value.as_raw());
        if ptr.is_null() {
            return Err(rquickjs::Error::Exception);
        }
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), len);
        let result = match std::str::from_utf8(bytes) {
            Ok(s) => f(s),
            Err(_) => f(&well_formed(bytes)),
        };
        qjs::JS_FreeCString(ctx.as_raw().as_ptr(), ptr);
        Ok(result)
    }
}

/// UTF-8 from QuickJS's encoding of a string with lone surrogates, which
/// writes each as a three-byte sequence (`ED A0..BF xx`): each becomes
/// U+FFFD.
fn well_formed(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let (good, bad) = rest.split_at(e.valid_up_to());
                // SAFETY: `from_utf8` checked the bytes before `valid_up_to`.
                out.push_str(unsafe { std::str::from_utf8_unchecked(good) });
                out.push('\u{fffd}');
                let skip = if bad.first() == Some(&0xed) && bad.len() >= 3 {
                    3
                } else {
                    e.error_len().unwrap_or(bad.len())
                };
                rest = &bad[skip.min(bad.len())..];
            }
        }
    }
    out
}

/// Parses `input` against an optional `base`, both JS strings.
fn parse_values(ctx: &Ctx<'_>, input: &Value<'_>, base: &Value<'_>) -> Result<Option<Url>> {
    with_usv(ctx, input, |input| {
        if base.is_undefined() {
            return Ok(parse(input, None));
        }
        with_usv(ctx, base, |base| parse(input, Some(base)))
    })?
}

/// The `Url` a `URL` object holds.
pub(crate) struct UrlRecord(RefCell<Url>);

impl<'js> Trace<'js> for UrlRecord {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

// SAFETY: `UrlRecord` holds no values bound to `'js`.
unsafe impl<'js> JsLifetime<'js> for UrlRecord {
    type Changed<'to> = UrlRecord;
}

impl<'js> JsClass<'js> for UrlRecord {
    const NAME: &'static str = "URLRecord";
    type Mutable = Readable;

    fn prototype(_ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
        // Records stay inside the glue, which never looks at their
        // properties.
        Ok(None)
    }

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

/// Adds the URL host functions to the prelude's `native` object.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, native: &Object<'js>) -> Result<()> {
    native.set(
        "urlNew",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             input: Value<'js>,
             base: Value<'js>|
             -> Result<Option<Class<'js, UrlRecord>>> {
                parse_values(&ctx, &input, &base)?
                    .map(|url| Class::instance(ctx.clone(), UrlRecord(RefCell::new(url))))
                    .transpose()
            },
        )?,
    )?;
    native.set(
        "urlGet",
        Function::new(ctx.clone(), |record: Class<'js, UrlRecord>, i: u32| {
            part(&record.borrow().0.borrow(), i)
        })?,
    )?;
    native.set(
        "urlSetPart",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, record: Class<'js, UrlRecord>, name: String, value: Value<'js>| {
                let record = record.borrow();
                with_usv(&ctx, &value, |value| {
                    set(&mut record.0.borrow_mut(), &name, value)
                })
            },
        )?,
    )?;
    native.set(
        "urlParse",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, input: Value<'js>, base: Value<'js>| {
                Ok::<_, rquickjs::Error>(parse_values(&ctx, &input, &base)?.map(|url| parts(&url)))
            },
        )?,
    )?;
    native.set(
        "urlHref",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, input: Value<'js>, base: Value<'js>| {
                Ok::<_, rquickjs::Error>(parse_values(&ctx, &input, &base)?.map(String::from))
            },
        )?,
    )?;
    native.set(
        "urlSet",
        Function::new(ctx.clone(), |href: String, part: String, value: String| {
            let mut url = Url::parse(&href).ok()?;
            set(&mut url, &part, &value).then(|| parts(&url))
        })?,
    )?;
    native.set(
        "queryParse",
        Function::new(ctx.clone(), |query: String| {
            form_urlencoded::parse(query.as_bytes())
                .flat_map(|(k, v)| [k.into_owned(), v.into_owned()])
                .collect::<Vec<_>>()
        })?,
    )?;
    native.set(
        "querySerialize",
        Function::new(ctx.clone(), |pairs: Vec<String>| {
            let mut out = form_urlencoded::Serializer::new(String::new());
            for pair in pairs.as_chunks::<2>().0 {
                out.append_pair(&pair[0], &pair[1]);
            }
            out.finish()
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_against_a_base() {
        let url = parse("../b?x=1#f", Some("https://a.example/dir/sub/page")).unwrap();
        assert_eq!(
            parts(&url),
            [
                "https://a.example/dir/b?x=1#f",
                "https://a.example",
                "https:",
                "",
                "",
                "a.example",
                "a.example",
                "",
                "/dir/b",
                "?x=1",
                "#f"
            ]
        );
        assert!(parse("/x", None).is_none());
        assert!(parse("/x", Some("not a url")).is_none());
        // The cached base is the one given, not the last good one.
        assert_eq!(
            parse("/y", Some("http://b.example/")).unwrap().as_str(),
            "http://b.example/y"
        );
        assert!(parse("/x", Some("not a url")).is_none());
    }

    #[test]
    fn setters() {
        let href = "http://example.com:8080/p?q#h";
        let with = |part: &str, value: &str| {
            let mut url = Url::parse(href).unwrap();
            set(&mut url, part, value).then(|| url.to_string())
        };
        assert_eq!(with("port", "80").unwrap(), "http://example.com/p?q#h");
        assert_eq!(with("search", "").unwrap(), "http://example.com:8080/p#h");
        assert_eq!(
            with("hash", "x y").unwrap(),
            "http://example.com:8080/p?q#x%20y"
        );
        assert_eq!(with("port", "nope").unwrap(), href);
        assert!(with("href", "nope").is_none());
    }

    #[test]
    fn lone_surrogates_become_replacement_characters() {
        // 'a', a lone high surrogate, 'é', a lone low surrogate.
        let bytes = b"a\xed\xa0\x80\xc3\xa9\xed\xb0\x80";
        assert_eq!(well_formed(bytes), "a\u{fffd}é\u{fffd}");
    }
}
