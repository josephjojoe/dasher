//! The id index: every element in the arena that has an `id` attribute,
//! found by the attribute's value.
//!
//! There is one index for the whole arena rather than one per tree scope.
//! It holds every element with an id, wherever it is: in the document, in a
//! shadow tree, in template contents, in an extra document or in a detached
//! tree. A lookup takes the elements with that id and keeps the ones under
//! the root it was asked about, so each tree scope still sees only its own
//! elements. Inserting and removing nodes therefore never touches the index,
//! however large the subtree moved: only creating an element with an id,
//! changing an `id` attribute and [releasing](crate::Document::release) an
//! element do. The price is an ancestor walk per candidate on lookup, which
//! is a few dozen steps on real pages.
//!
//! Entries are keyed by a 32-bit hash of the ASCII-lowercased value, so the
//! same entry serves exact lookups (`getElementById`, `#id` in standards
//! mode) and ASCII case-insensitive ones (`#id` in quirks mode). Values are
//! not stored: every candidate is checked against its element's attribute,
//! so a hash collision costs a comparison, not a wrong answer. The hash is
//! keyed per document, so a page cannot make its ids collide on purpose.
//!
//! The index costs 10 to 20 bytes per element with an id (an 8-byte entry
//! and a control byte, in a table between 7/16 and 7/8 full), and nothing
//! for elements without one. `tests/memory.rs` measures it.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher, RandomState};

use crate::node::NodeId;

#[derive(Default)]
pub(crate) struct IdIndex {
    hasher: RandomState,
    /// Key -> one element with an id of that key.
    pub(crate) first: HashMap<u32, NodeId, BuildHasherDefault<Prehashed>>,
    /// Key -> the other elements with an id of that key, for ids that
    /// repeat (or collide). Usually empty.
    pub(crate) more: HashMap<u32, Vec<NodeId>, BuildHasherDefault<Prehashed>>,
}

impl IdIndex {
    /// The key of an id: a hash of its ASCII-lowercased bytes.
    pub(crate) fn key(&self, id: &str) -> u32 {
        let mut h = self.hasher.build_hasher();
        let mut buf = [0u8; 32];
        for chunk in id.as_bytes().chunks(buf.len()) {
            let buf = &mut buf[..chunk.len()];
            buf.copy_from_slice(chunk);
            buf.make_ascii_lowercase();
            h.write(buf);
        }
        let hash = h.finish();
        (hash ^ (hash >> 32)) as u32
    }

    pub(crate) fn insert(&mut self, id: &str, node: NodeId) {
        let key = self.key(id);
        match self.first.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(node);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                self.more.entry(key).or_default().push(node);
            }
        }
    }

    pub(crate) fn remove(&mut self, id: &str, node: NodeId) {
        let key = self.key(id);
        let Some(&first) = self.first.get(&key) else {
            debug_assert!(false, "id {id:?} not indexed");
            return;
        };
        let more = self.more.get_mut(&key);
        if first == node {
            match more {
                Some(more) => {
                    let next = more.pop().expect("no empty lists are kept");
                    if more.is_empty() {
                        self.more.remove(&key);
                    }
                    self.first.insert(key, next);
                }
                None => {
                    self.first.remove(&key);
                }
            }
        } else if let Some(more) = more {
            if let Some(i) = more.iter().position(|&n| n == node) {
                more.swap_remove(i);
            } else {
                debug_assert!(false, "{node:?} not indexed under {id:?}");
            }
            if more.is_empty() {
                self.more.remove(&key);
            } else if more.len() * 4 < more.capacity() {
                more.shrink_to_fit();
            }
        } else {
            debug_assert!(false, "{node:?} not indexed under {id:?}");
        }
    }

    /// The elements whose id may be `id`, ignoring ASCII case, in no
    /// particular order. Callers check each one's actual id.
    pub(crate) fn candidates(&self, id: &str) -> Candidates<'_> {
        let key = self.key(id);
        Candidates {
            first: self.first.get(&key).copied(),
            more: if self.more.is_empty() {
                [].iter()
            } else {
                self.more.get(&key).map_or([].iter(), |v| v.iter())
            },
        }
    }

    /// The number of indexed elements.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.first.len() + self.more.values().map(Vec::len).sum::<usize>()
    }
}

/// See [`IdIndex::candidates`].
#[derive(Clone)]
pub(crate) struct Candidates<'a> {
    first: Option<NodeId>,
    more: std::slice::Iter<'a, NodeId>,
}

impl Iterator for Candidates<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        self.first.take().or_else(|| self.more.next().copied())
    }
}

/// A hasher for keys that are already hashes. It only spreads the 32 bits
/// over 64: the table takes its bucket from the low bits and a tag from the
/// top 7, and a zero-extended key would give every entry the same tag.
#[derive(Default)]
pub(crate) struct Prehashed(u64);

impl Hasher for Prehashed {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

#[cfg(test)]
mod tests {
    use crate::Document;

    #[test]
    fn released_and_changed_ids_leave_nothing_behind() {
        let mut doc = Document::parse("<div id=a><p id=a></p><p id=b></p></div>");
        assert_eq!(doc.ids.len(), 3);
        let root = doc.root();
        let div = doc.get_element_by_id(root, "a").unwrap();
        doc.set_attribute(div, "id", "c");
        doc.remove(div);
        assert_eq!(doc.ids.len(), 3);
        let copy = doc.clone_node(div, true);
        assert_eq!(doc.ids.len(), 6);
        doc.release(copy);
        doc.release(div);
        assert_eq!(doc.ids.len(), 0);
        assert!(doc.ids.first.is_empty() && doc.ids.more.is_empty());
    }

    #[test]
    fn keys_ignore_ascii_case_only() {
        let doc = Document::new();
        let ids = &doc.ids;
        assert_eq!(ids.key("Main"), ids.key("mAIN"));
        assert_ne!(ids.key("main"), ids.key("mains"));
        let long = "x".repeat(100);
        assert_eq!(ids.key(&long), ids.key(&long.to_ascii_uppercase()));
        assert_ne!(ids.key("é"), ids.key("É"));
    }
}
