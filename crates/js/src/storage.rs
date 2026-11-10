//! In-memory Web Storage behind `localStorage` and `sessionStorage`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

use rquickjs::{Ctx, Function, Object};
use url::Url;

/// Default storage quota per origin: 5 MiB, as in browsers.
pub const DEFAULT_STORAGE_QUOTA: usize = 5 * 1024 * 1024;

/// Storage was full: storing the item would take the origin past its quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaExceeded;

impl fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("storage quota exceeded")
    }
}

impl std::error::Error for QuotaExceeded {}

/// Key/value storage for `localStorage` or `sessionStorage`, partitioned by
/// origin and kept in memory only.
///
/// Clones share the same data, so a host can give several pages one store
/// (for example every page of a scraping session) and can read or seed
/// items from Rust. Size is measured the way browsers store strings, two
/// bytes per UTF-16 code unit of each key and value, and each origin is
/// capped at the store's quota.
#[derive(Clone)]
pub struct StorageStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    quota: usize,
    areas: HashMap<String, Area>,
}

#[derive(Default)]
struct Area {
    /// Sorted by key, which also fixes the order `key(n)` uses.
    items: BTreeMap<String, String>,
    bytes: usize,
}

fn cost(text: &str) -> usize {
    text.encode_utf16().count() * 2
}

impl Default for StorageStore {
    fn default() -> Self {
        Self::with_quota(DEFAULT_STORAGE_QUOTA)
    }
}

impl fmt::Debug for StorageStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.lock();
        f.debug_struct("StorageStore")
            .field("quota", &inner.quota)
            .field("origins", &inner.areas.len())
            .finish()
    }
}

impl StorageStore {
    /// An empty store with the default quota.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty store allowing each origin `quota` bytes.
    pub fn with_quota(quota: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                quota,
                areas: HashMap::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Nothing panics while the lock is held, so the data is consistent
        // even if the lock was poisoned.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The value stored under `key` for `origin`, which is an ASCII-serialized
    /// origin such as `https://example.com`.
    pub fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.lock().areas.get(origin)?.items.get(key).cloned()
    }

    /// Stores `value` under `key` for `origin`, as `setItem` does.
    pub fn set(&self, origin: &str, key: &str, value: &str) -> Result<(), QuotaExceeded> {
        let mut inner = self.lock();
        let quota = inner.quota;
        let area = inner.areas.entry(origin.to_owned()).or_default();
        let old = area.items.get(key).map_or(0, |v| cost(key) + cost(v));
        let new = cost(key) + cost(value);
        let bytes = area.bytes - old + new;
        if new > old && bytes > quota {
            return Err(QuotaExceeded);
        }
        area.bytes = bytes;
        area.items.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    /// Removes `key` for `origin`, returning the value it had.
    pub fn remove(&self, origin: &str, key: &str) -> Option<String> {
        let mut inner = self.lock();
        let area = inner.areas.get_mut(origin)?;
        let value = area.items.remove(key)?;
        area.bytes -= cost(key) + cost(&value);
        if area.items.is_empty() {
            inner.areas.remove(origin);
        }
        Some(value)
    }

    /// Removes everything stored for `origin`.
    pub fn clear(&self, origin: &str) {
        self.lock().areas.remove(origin);
    }

    /// Every item stored for `origin`, sorted by key.
    pub fn items(&self, origin: &str) -> Vec<(String, String)> {
        self.lock().areas.get(origin).map_or_else(Vec::new, |area| {
            area.items
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
    }

    /// The origins that have something stored.
    pub fn origins(&self) -> Vec<String> {
        self.lock().areas.keys().cloned().collect()
    }

    fn len(&self, origin: &str) -> usize {
        self.lock().areas.get(origin).map_or(0, |a| a.items.len())
    }

    fn key(&self, origin: &str, index: usize) -> Option<String> {
        self.lock()
            .areas
            .get(origin)?
            .items
            .keys()
            .nth(index)
            .cloned()
    }

    fn keys(&self, origin: &str) -> Vec<String> {
        self.lock()
            .areas
            .get(origin)
            .map_or_else(Vec::new, |a| a.items.keys().cloned().collect())
    }
}

/// The two storage areas a page sees, bound to its origin.
struct Areas {
    origin: String,
    stores: [StorageStore; 2],
}

impl Areas {
    fn store(&self, area: u32) -> &StorageStore {
        &self.stores[usize::from(area != 0)]
    }
}

/// Adds the storage host functions to the prelude's `native` object. Area 0
/// is `localStorage` and area 1 `sessionStorage`.
///
/// Pages with an opaque origin (`about:blank`, `data:`) get private stores
/// of their own, since nothing else can share their origin.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    url: &Url,
    local: &StorageStore,
    session: &StorageStore,
) -> rquickjs::Result<()> {
    let origin = url.origin();
    let areas = Rc::new(if origin.is_tuple() {
        Areas {
            origin: origin.ascii_serialization(),
            stores: [local.clone(), session.clone()],
        }
    } else {
        Areas {
            origin: String::from("null"),
            stores: [StorageStore::new(), StorageStore::new()],
        }
    });

    let a = Rc::clone(&areas);
    native.set(
        "storageLength",
        Function::new(ctx.clone(), move |area: u32| a.store(area).len(&a.origin))?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageKey",
        Function::new(ctx.clone(), move |area: u32, index: f64| {
            if index < 0.0 || !index.is_finite() {
                return None;
            }
            // Already an integer from WebIDL `unsigned long` conversion.
            a.store(area).key(&a.origin, index as usize)
        })?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageKeys",
        Function::new(ctx.clone(), move |area: u32| a.store(area).keys(&a.origin))?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageGet",
        Function::new(ctx.clone(), move |area: u32, key: String| {
            a.store(area).get(&a.origin, &key)
        })?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageSet",
        Function::new(ctx.clone(), move |area: u32, key: String, value: String| {
            a.store(area).set(&a.origin, &key, &value).is_ok()
        })?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageRemove",
        Function::new(ctx.clone(), move |area: u32, key: String| {
            a.store(area).remove(&a.origin, &key);
        })?,
    )?;
    let a = Rc::clone(&areas);
    native.set(
        "storageClear",
        Function::new(ctx.clone(), move |area: u32| a.store(area).clear(&a.origin))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_partitioned_by_origin_and_sorted() {
        let store = StorageStore::new();
        store.set("https://a.test", "b", "2").unwrap();
        store.set("https://a.test", "a", "1").unwrap();
        store.set("https://b.test", "a", "other").unwrap();
        assert_eq!(
            store.items("https://a.test"),
            [("a".into(), "1".into()), ("b".into(), "2".into())]
        );
        assert_eq!(store.get("https://b.test", "a").as_deref(), Some("other"));
        assert_eq!(store.key("https://a.test", 1).as_deref(), Some("b"));
        assert_eq!(store.remove("https://a.test", "a").as_deref(), Some("1"));
        store.clear("https://a.test");
        assert_eq!(store.len("https://a.test"), 0);
        assert_eq!(store.origins(), ["https://b.test"]);
    }

    #[test]
    fn quota_counts_utf16_bytes_per_origin() {
        let store = StorageStore::with_quota(10);
        store.set("o", "k", "1234").unwrap(); // 2 + 8 bytes
        assert_eq!(store.set("o", "k", "12345"), Err(QuotaExceeded));
        assert_eq!(store.set("o", "x", ""), Err(QuotaExceeded));
        // Shrinking or replacing within the quota is fine.
        store.set("o", "k", "12").unwrap();
        store.set("o", "x", "").unwrap();
        store.set("other", "k", "1234").unwrap();
        assert_eq!(store.get("o", "k").as_deref(), Some("12"));
    }

    #[test]
    fn clones_share_data() {
        let a = StorageStore::new();
        let b = a.clone();
        a.set("o", "k", "v").unwrap();
        assert_eq!(b.get("o", "k").as_deref(), Some("v"));
    }
}
