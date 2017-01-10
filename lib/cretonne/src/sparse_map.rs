//! Sparse mapping of entity references to larger value types.
//!
//! This module provides a `SparseMap` data structure which implements a sparse mapping from an
//! `EntityRef` key to a value type that may be on the larger side. This implementation is based on
//! the paper:
//!
//! > Briggs, Torczon, *An efficient representation for sparse sets*,
//!   ACM Letters on Programming Languages and Systems, Volume 2, Issue 1-4, March-Dec. 1993.
//!
//! A `SparseMap<K, V>` map provides:
//!
//! - Memory usage equivalent to `EntityMap<K, u32>` + `Vec<V>`, so much smaller than
//!   `EntityMap<K, V>` for sparse mappings of larger `V` types.
//! - Constant time lookup, slightly slower than `EntityMap`.
//! - A very fast, constant time `clear()` operation.
//! - Fast insert and erase operations.
//! - Stable iteration that is as fast as a `Vec<V>`.
//!
//! # Compared to `EntityMap`
//!
//! When should we use a `SparseMap` instead of a secondary `EntityMap`? First of all, `SparseMap`
//! does not provide the functionality of a primary `EntityMap` which can allocate and assign
//! entity references to objects as they are pushed onto the map. It is only the secondary
//! entity maps that can be replaced with a `SparseMap`.
//!
//! - A secondary entity map requires its values to implement `Default`, and it is a bit loose
//!   about creating new mappings to the default value. It doesn't distinguish clearly between an
//!   unmapped key and one that maps to the default value. `SparseMap` does not require `Default`
//!   values, and it tracks accurately if a key has been mapped or not.
//! - Iterating over the contants of an `EntityMap` is linear in the size of the *key space*, while
//!   iterating over a `SparseMap` is linear in the number of elements in the mapping. This is an
//!   advantage precisely when the mapping is sparse.
//! - `SparseMap::clear()` is constant time and super-fast. `EntityMap::clear()` is linear in the
//!   size of the key space. (Or, rather the required `resize()` call following the `clear()` is).
//! - `SparseMap` requires the values to implement `SparseMapValue<K>` which means that they must
//!   contain their own key.

use entity_map::{EntityRef, EntityMap};
use std::mem;
use std::u32;

/// Trait for extracting keys from values stored in a `SparseMap`.
///
/// All values stored in a `SparseMap` must keep track of their own key in the map and implement
/// this trait to provide access to the key.
pub trait SparseMapValue<K> {
    /// Get the key of this sparse map value. This key is not alowed to change while the value
    /// is a member of the map.
    fn key(&self) -> K;
}

/// A sparse mapping of entity references.
pub struct SparseMap<K, V>
    where K: EntityRef,
          V: SparseMapValue<K>
{
    sparse: EntityMap<K, u32>,
    dense: Vec<V>,
}

impl<K, V> SparseMap<K, V>
    where K: EntityRef,
          V: SparseMapValue<K>
{
    /// Create a new empty mapping.
    pub fn new() -> Self {
        SparseMap {
            sparse: EntityMap::new(),
            dense: Vec::new(),
        }
    }

    /// Returns the number of elements in the map.
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    /// Returns true is the map contains no elements.
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get(&self, key: K) -> Option<&V> {
        if let Some(idx) = self.sparse.get(key).cloned() {
            if let Some(entry) = self.dense.get(idx as usize) {
                if entry.key() == key {
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Returns a mutable reference to the value corresponding to the key.
    ///
    /// Note that the returned value must not be mutated in a way that would change its key. This
    /// would invalidate the sparse set data structure.
    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        if let Some(idx) = self.sparse.get(key).cloned() {
            if let Some(entry) = self.dense.get_mut(idx as usize) {
                if entry.key() == key {
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Return the index into `dense` of the value corresponding to `key`.
    fn index(&self, key: K) -> Option<usize> {
        if let Some(idx) = self.sparse.get(key).cloned() {
            let idx = idx as usize;
            if let Some(entry) = self.dense.get(idx) {
                if entry.key() == key {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Insert a value into the map.
    ///
    /// If the map did not have this key present, `None` is returned.
    ///
    /// If the map did have this key present, the value is updated, and the old value is returned.
    ///
    /// It is not necessary to provide a key since the value knows its own key already.
    pub fn insert(&mut self, value: V) -> Option<V> {
        let key = value.key();

        // Replace the existing entry for `key` if there is one.
        if let Some(entry) = self.get_mut(key) {
            return Some(mem::replace(entry, value));
        }

        // There was no previous entry for `key`. Add it to the end of `dense`.
        let idx = self.dense.len();
        assert!(idx <= u32::MAX as usize, "SparseMap overflow");
        self.dense.push(value);
        *self.sparse.ensure(key) = idx as u32;
        None
    }

    /// Remove a value from the map and return it.
    pub fn remove(&mut self, key: K) -> Option<V> {
        if let Some(idx) = self.index(key) {
            let back = self.dense.pop().unwrap();

            // Are we popping the back of `dense`?
            if idx == self.dense.len() {
                return Some(back);
            }

            // We're removing an element from the middle of `dense`.
            // Replace the element at `idx` with the back of `dense`.
            // Repair `sparse` first.
            self.sparse[back.key()] = idx as u32;
            return Some(mem::replace(&mut self.dense[idx], back));
        }

        // Nothing to remove.
        None
    }
}

/// Any `EntityRef` can be used as a sparse map value representing itself.
impl<T> SparseMapValue<T> for T
    where T: EntityRef
{
    fn key(&self) -> T {
        *self
    }
}

/// A sparse set of entity references.
///
/// Any type that implements `EntityRef` can be used as a sparse set value too.
pub type SparseSet<T> = SparseMap<T, T>;

#[cfg(test)]
mod tests {
    use super::*;
    use entity_map::EntityRef;
    use ir::Inst;

    // Mock key-value object for testing.
    #[derive(PartialEq, Eq, Debug)]
    struct Obj(Inst, &'static str);

    impl SparseMapValue<Inst> for Obj {
        fn key(&self) -> Inst {
            self.0
        }
    }

    #[test]
    fn empty_immutable_map() {
        let i1 = Inst::new(1);
        let map: SparseMap<Inst, Obj> = SparseMap::new();

        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(i1), None);
    }

    #[test]
    fn single_entry() {
        let i0 = Inst::new(0);
        let i1 = Inst::new(1);
        let i2 = Inst::new(2);
        let mut map = SparseMap::new();

        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(i1), None);
        assert_eq!(map.get_mut(i1), None);
        assert_eq!(map.remove(i1), None);

        assert_eq!(map.insert(Obj(i1, "hi")), None);
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(i0), None);
        assert_eq!(map.get(i1), Some(&Obj(i1, "hi")));
        assert_eq!(map.get(i2), None);
        assert_eq!(map.get_mut(i0), None);
        assert_eq!(map.get_mut(i1), Some(&mut Obj(i1, "hi")));
        assert_eq!(map.get_mut(i2), None);

        assert_eq!(map.remove(i0), None);
        assert_eq!(map.remove(i2), None);
        assert_eq!(map.remove(i1), Some(Obj(i1, "hi")));
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(i1), None);
        assert_eq!(map.get_mut(i1), None);
        assert_eq!(map.remove(i0), None);
        assert_eq!(map.remove(i1), None);
        assert_eq!(map.remove(i2), None);
    }

    #[test]
    fn multiple_entries() {
        let i0 = Inst::new(0);
        let i1 = Inst::new(1);
        let i2 = Inst::new(2);
        let i3 = Inst::new(3);
        let mut map = SparseMap::new();

        assert_eq!(map.insert(Obj(i2, "foo")), None);
        assert_eq!(map.insert(Obj(i1, "bar")), None);
        assert_eq!(map.insert(Obj(i0, "baz")), None);

        assert_eq!(map.len(), 3);
        assert_eq!(map.get(i0), Some(&Obj(i0, "baz")));
        assert_eq!(map.get(i1), Some(&Obj(i1, "bar")));
        assert_eq!(map.get(i2), Some(&Obj(i2, "foo")));
        assert_eq!(map.get(i3), None);

        // Remove front object, causing back to be swapped down.
        assert_eq!(map.remove(i1), Some(Obj(i1, "bar")));
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(i0), Some(&Obj(i0, "baz")));
        assert_eq!(map.get(i1), None);
        assert_eq!(map.get(i2), Some(&Obj(i2, "foo")));
        assert_eq!(map.get(i3), None);

        // Reinsert something at a previously used key.
        assert_eq!(map.insert(Obj(i1, "barbar")), None);
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(i0), Some(&Obj(i0, "baz")));
        assert_eq!(map.get(i1), Some(&Obj(i1, "barbar")));
        assert_eq!(map.get(i2), Some(&Obj(i2, "foo")));
        assert_eq!(map.get(i3), None);

        // Replace an entry.
        assert_eq!(map.insert(Obj(i0, "bazbaz")), Some(Obj(i0, "baz")));
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(i0), Some(&Obj(i0, "bazbaz")));
        assert_eq!(map.get(i1), Some(&Obj(i1, "barbar")));
        assert_eq!(map.get(i2), Some(&Obj(i2, "foo")));
        assert_eq!(map.get(i3), None);
    }

    #[test]
    fn entity_set() {
        let i0 = Inst::new(0);
        let i1 = Inst::new(1);
        let mut set = SparseSet::new();

        assert_eq!(set.insert(i0), None);
        assert_eq!(set.insert(i0), Some(i0));
        assert_eq!(set.insert(i1), None);
        assert_eq!(set.get(i0), Some(&i0));
        assert_eq!(set.get(i1), Some(&i1));
    }
}