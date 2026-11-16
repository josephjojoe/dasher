//! Resolving module specifiers, with import maps.
//!
//! <https://html.spec.whatwg.org/#resolving-a-module-specifier>
//! <https://html.spec.whatwg.org/#import-maps>

use url::Url;

/// One import map entry: a specifier (or prefix ending in `/`) and the URL
/// it maps to. `None` blocks the specifier: resolving it fails.
type Entry = (String, Option<Url>);

/// The import maps a document has registered, merged.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ImportMap {
    /// Sorted with the most specific (code-unit greatest) key first.
    imports: Vec<Entry>,
    /// Scope prefixes with their entries, most specific scope first.
    scopes: Vec<(String, Vec<Entry>)>,
}

/// Specifiers and addresses as read from an import map's JSON. Addresses
/// that are not strings are `None`.
pub(crate) type RawEntries = Vec<(String, Option<String>)>;

/// An import map as read from JSON, before its URLs are resolved.
#[derive(Debug, Default)]
pub(crate) struct RawImportMap {
    pub(crate) imports: RawEntries,
    pub(crate) scopes: Vec<(String, RawEntries)>,
}

impl ImportMap {
    /// Resolves the URLs of an import map from a script whose base URL is
    /// `base`. Entries that cannot be used are left out (or block their
    /// specifier), with a warning for the console.
    /// <https://html.spec.whatwg.org/#parse-an-import-map-string>
    pub(crate) fn parse(raw: RawImportMap, base: &Url, warnings: &mut Vec<String>) -> Self {
        let imports = normalize_entries(raw.imports, base, warnings);
        let mut scopes = Vec::new();
        for (prefix, entries) in raw.scopes {
            let Ok(prefix_url) = base.join(&prefix) else {
                warnings.push(format!(
                    "Ignored import map scope {prefix:?}: not a valid URL"
                ));
                continue;
            };
            scopes.push((
                String::from(prefix_url),
                normalize_entries(entries, base, warnings),
            ));
        }
        sort(&mut scopes);
        Self { imports, scopes }
    }

    /// Adds the entries of `other` that this map does not have yet.
    /// <https://html.spec.whatwg.org/#merge-existing-and-new-import-maps>
    pub(crate) fn merge(&mut self, other: ImportMap) {
        merge_entries(&mut self.imports, other.imports);
        for (prefix, entries) in other.scopes {
            match self.scopes.iter_mut().find(|(p, _)| *p == prefix) {
                Some((_, existing)) => merge_entries(existing, entries),
                None => self.scopes.push((prefix, entries)),
            }
        }
        sort(&mut self.scopes);
    }

    /// Resolves `specifier`, imported by a script whose base URL is `base`.
    /// <https://html.spec.whatwg.org/#resolve-a-module-specifier>
    pub(crate) fn resolve(&self, specifier: &str, base: &Url) -> Result<Url, String> {
        let as_url = resolve_url_like(specifier, base);
        let normalized = as_url.as_ref().map_or(specifier, Url::as_str);
        let base_str = base.as_str();
        for (prefix, entries) in &self.scopes {
            let in_scope = prefix == base_str
                || (prefix.ends_with('/') && base_str.starts_with(prefix.as_str()));
            if in_scope && let Some(url) = match_entries(normalized, as_url.as_ref(), entries)? {
                return Ok(url);
            }
        }
        if let Some(url) = match_entries(normalized, as_url.as_ref(), &self.imports)? {
            return Ok(url);
        }
        as_url.ok_or_else(|| {
            format!(
                "Failed to resolve module specifier \"{specifier}\". Relative references must \
                 start with either \"/\", \"./\", or \"../\"."
            )
        })
    }
}

/// <https://html.spec.whatwg.org/#resolving-a-url-like-module-specifier>
fn resolve_url_like(specifier: &str, base: &Url) -> Option<Url> {
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        base.join(specifier).ok()
    } else {
        Url::parse(specifier).ok()
    }
}

/// <https://html.spec.whatwg.org/#resolving-an-imports-match>
fn match_entries(
    normalized: &str,
    as_url: Option<&Url>,
    entries: &[Entry],
) -> Result<Option<Url>, String> {
    let blocked = || format!("Import of \"{normalized}\" is blocked by the import map.");
    for (key, target) in entries {
        if key == normalized {
            return target.clone().map(Some).ok_or_else(blocked);
        }
        let special = as_url.is_none_or(|u| u.is_special());
        if key.ends_with('/') && special {
            let Some(after) = normalized.strip_prefix(key.as_str()) else {
                continue;
            };
            let target = target.as_ref().ok_or_else(blocked)?;
            let url = target.join(after).map_err(|_| blocked())?;
            if !url.as_str().starts_with(target.as_str()) {
                return Err(format!(
                    "Import of \"{normalized}\" backtracks above its import map prefix."
                ));
            }
            return Ok(Some(url));
        }
    }
    Ok(None)
}

/// <https://html.spec.whatwg.org/#sorting-and-normalizing-a-module-specifier-map>
fn normalize_entries(raw: RawEntries, base: &Url, warnings: &mut Vec<String>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::with_capacity(raw.len());
    for (key, value) in raw {
        if key.is_empty() {
            warnings.push(String::from("Ignored an empty import map specifier key"));
            continue;
        }
        let key = resolve_url_like(&key, base).map_or(key, String::from);
        let Some(value) = value else {
            warnings.push(format!("Import map address for {key:?} is not a string"));
            entries.push((key, None));
            continue;
        };
        let Some(address) = resolve_url_like(&value, base) else {
            warnings.push(format!("Import map address {value:?} is not a valid URL"));
            entries.push((key, None));
            continue;
        };
        if key.ends_with('/') && !address.as_str().ends_with('/') {
            warnings.push(format!(
                "Import map address {value:?} for the prefix {key:?} must end in \"/\""
            ));
            entries.push((key, None));
            continue;
        }
        // A key given twice keeps its last value, as in a JSON object.
        entries.retain(|(k, _)| *k != key);
        entries.push((key, Some(address)));
    }
    sort(&mut entries);
    entries
}

fn merge_entries(existing: &mut Vec<Entry>, new: Vec<Entry>) {
    for (key, value) in new {
        if !existing.iter().any(|(k, _)| *k == key) {
            existing.push((key, value));
        }
    }
    sort(existing);
}

/// Most specific key first: descending code-unit order puts every prefix
/// after the longer keys it is a prefix of.
fn sort<T>(entries: &mut [(String, T)]) {
    entries.sort_by(|(a, _), (b, _)| b.encode_utf16().cmp(a.encode_utf16()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    type Pairs<'a> = &'a [(&'a str, Option<&'a str>)];

    fn raw(imports: Pairs<'_>) -> RawEntries {
        imports
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
            .collect()
    }

    fn map(imports: Pairs<'_>, scopes: &[(&str, Pairs<'_>)]) -> ImportMap {
        let raw = RawImportMap {
            imports: raw(imports),
            scopes: scopes
                .iter()
                .map(|(p, e)| ((*p).to_owned(), raw(e)))
                .collect(),
        };
        let mut warnings = Vec::new();
        ImportMap::parse(
            raw,
            &url("https://example.com/app/index.html"),
            &mut warnings,
        )
    }

    #[test]
    fn without_a_map() {
        let map = ImportMap::default();
        let base = url("https://example.com/app/main.js");
        let ok = |s: &str| map.resolve(s, &base).unwrap().to_string();
        assert_eq!(ok("./a.js"), "https://example.com/app/a.js");
        assert_eq!(ok("../b.js?x#y"), "https://example.com/b.js?x#y");
        assert_eq!(ok("/c.js"), "https://example.com/c.js");
        assert_eq!(ok("//cdn.example.net/d.js"), "https://cdn.example.net/d.js");
        assert_eq!(
            ok("https://cdn.example.net/e.js"),
            "https://cdn.example.net/e.js"
        );
        assert_eq!(ok("data:text/javascript,1"), "data:text/javascript,1");
        let err = map.resolve("react", &base).unwrap_err();
        assert!(err.contains("\"react\""), "{err}");
        assert!(map.resolve("a.js", &base).is_err());
    }

    #[test]
    fn bare_specifiers_and_prefixes() {
        let map = map(
            &[
                ("react", Some("/vendor/react.js")),
                ("lib/", Some("https://cdn.example.net/lib/v2/")),
                ("lib/special", Some("./special.js")),
                ("blocked", None),
                ("bad-prefix/", Some("/no-slash")),
                ("https://example.com/app/old.js", Some("/app/new.js")),
            ],
            &[],
        );
        let base = url("https://example.com/app/main.js");
        let ok = |s: &str| map.resolve(s, &base).unwrap().to_string();
        assert_eq!(ok("react"), "https://example.com/vendor/react.js");
        assert_eq!(ok("lib/x/y.js"), "https://cdn.example.net/lib/v2/x/y.js");
        assert_eq!(ok("lib/special"), "https://example.com/app/special.js");
        // URL-like specifiers are mapped too, once resolved.
        assert_eq!(ok("./old.js"), "https://example.com/app/new.js");
        assert!(map.resolve("blocked", &base).is_err());
        assert!(map.resolve("bad-prefix/a.js", &base).is_err());
        assert!(map.resolve("lib/../../escape.js", &base).is_err());
    }

    #[test]
    fn scopes() {
        let map = map(
            &[("dep", Some("/dep/v1.js"))],
            &[
                ("/legacy/", &[("dep", Some("/dep/v0.js"))]),
                ("/legacy/very/", &[("dep", Some("/dep/v-old.js"))]),
                ("/exact.js", &[("dep", Some("/dep/exact.js"))]),
            ],
        );
        let resolve = |base: &str| map.resolve("dep", &url(base)).unwrap().path().to_owned();
        assert_eq!(resolve("https://example.com/app/main.js"), "/dep/v1.js");
        assert_eq!(resolve("https://example.com/legacy/main.js"), "/dep/v0.js");
        assert_eq!(
            resolve("https://example.com/legacy/very/old.js"),
            "/dep/v-old.js"
        );
        assert_eq!(resolve("https://example.com/exact.js"), "/dep/exact.js");
        assert_eq!(resolve("https://example.com/exact.jsx"), "/dep/v1.js");
    }

    #[test]
    fn merging_keeps_existing_entries() {
        let mut first = map(
            &[("a", Some("/a1.js"))],
            &[("/s/", &[("x", Some("/x1.js"))])],
        );
        first.merge(map(
            &[("a", Some("/a2.js")), ("b", Some("/b2.js"))],
            &[("/s/", &[("x", Some("/x2.js")), ("y", Some("/y2.js"))])],
        ));
        let base = url("https://example.com/s/main.js");
        let path = |s: &str| first.resolve(s, &base).unwrap().path().to_owned();
        assert_eq!(path("a"), "/a1.js");
        assert_eq!(path("b"), "/b2.js");
        assert_eq!(path("x"), "/x1.js");
        assert_eq!(path("y"), "/y2.js");
    }

    #[test]
    fn invalid_entries_warn() {
        let raw = RawImportMap {
            imports: raw(&[("", Some("/a.js")), ("n", None), ("p/", Some("/q"))]),
            scopes: vec![("https://[bad".into(), Vec::new())],
        };
        let mut warnings = Vec::new();
        let map = ImportMap::parse(raw, &url("https://example.com/"), &mut warnings);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(map.scopes.is_empty());
        assert!(map.resolve("n", &url("https://example.com/")).is_err());
    }
}
