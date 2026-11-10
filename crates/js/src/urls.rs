//! WHATWG URL parsing for `URL` and `URLSearchParams`, via the `url` crate.
//!
//! The glue keeps a URL as its serialized `href` and asks Rust to parse it
//! or to apply a setter, getting every component back at once.

use rquickjs::{Ctx, Function, Object};
use url::{Url, form_urlencoded, quirks};

/// A URL's components in the order the glue reads them: `href`, `origin`,
/// `protocol`, `username`, `password`, `host`, `hostname`, `port`,
/// `pathname`, `search`, `hash`.
pub(crate) fn parts(url: &Url) -> Vec<String> {
    [
        quirks::href(url),
        &quirks::origin(url),
        quirks::protocol(url),
        quirks::username(url),
        quirks::password(url),
        quirks::host(url),
        quirks::hostname(url),
        quirks::port(url),
        quirks::pathname(url),
        quirks::search(url),
        quirks::hash(url),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Parses `input` against an optional `base`, as `new URL(input, base)`.
pub(crate) fn parse(input: &str, base: Option<&str>) -> Option<Url> {
    match base {
        Some(base) => Url::parse(base).ok()?.join(input).ok(),
        None => Url::parse(input).ok(),
    }
}

/// Applies the `URL` attribute setter `part`. Returns `None` only when the
/// `href` setter fails, which throws; other setters ignore bad input.
fn set(href: &str, part: &str, value: &str) -> Option<Url> {
    let mut url = Url::parse(href).ok()?;
    let result = match part {
        "href" => quirks::set_href(&mut url, value).map_err(drop),
        "protocol" => quirks::set_protocol(&mut url, value),
        "username" => quirks::set_username(&mut url, value),
        "password" => quirks::set_password(&mut url, value),
        "host" => quirks::set_host(&mut url, value),
        "hostname" => quirks::set_hostname(&mut url, value),
        "port" => quirks::set_port(&mut url, value),
        "pathname" => {
            quirks::set_pathname(&mut url, value);
            Ok(())
        }
        "search" => {
            quirks::set_search(&mut url, value);
            Ok(())
        }
        "hash" => {
            quirks::set_hash(&mut url, value);
            Ok(())
        }
        _ => Ok(()),
    };
    match (part, result) {
        ("href", Err(())) => None,
        _ => Some(url),
    }
}

/// Adds the URL host functions to the prelude's `native` object.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, native: &Object<'js>) -> rquickjs::Result<()> {
    native.set(
        "urlParse",
        Function::new(ctx.clone(), |input: String, base: Option<String>| {
            parse(&input, base.as_deref()).map(|url| parts(&url))
        })?,
    )?;
    native.set(
        "urlSet",
        Function::new(ctx.clone(), |href: String, part: String, value: String| {
            set(&href, &part, &value).map(|url| parts(&url))
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
            for pair in pairs.chunks_exact(2) {
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
    }

    #[test]
    fn setters() {
        let href = "http://example.com:8080/p?q#h";
        assert_eq!(
            set(href, "port", "80").unwrap().as_str(),
            "http://example.com/p?q#h"
        );
        assert_eq!(
            set(href, "search", "").unwrap().as_str(),
            "http://example.com:8080/p#h"
        );
        assert_eq!(
            set(href, "hash", "x y").unwrap().as_str(),
            "http://example.com:8080/p?q#x%20y"
        );
        assert_eq!(set(href, "port", "nope").unwrap().as_str(), href);
        assert!(set(href, "href", "nope").is_none());
    }
}
