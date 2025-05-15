use rustc_hash::{FxBuildHasher, FxHashMap as HashMap};
use std::hash::Hash;

use crate::calculate_hash;

#[derive(Clone, Debug, PartialEq)]
pub struct Interner<V>
where
    V: Hash + Eq,
{
    uniq: HashMap<u64, usize>,
    storage: Vec<V>,
}

impl<V: Eq + Hash> Default for Interner<V> {
    fn default() -> Self {
        Interner {
            uniq: HashMap::with_capacity_and_hasher(8, FxBuildHasher),
            storage: Vec::with_capacity(8),
        }
    }
}

impl<V: Hash + Eq + std::fmt::Debug> Interner<V> {
    pub fn intern(&mut self, value: V) -> usize {
        let hash = calculate_hash(&value);

        *self.uniq.entry(hash).or_insert_with(|| {
            self.storage.push(value);

            self.storage.len() - 1
        })
    }

    pub fn replace(&mut self, index: usize, value: V) {
        assert!(index < self.storage.len());

        self.storage[index] = value;
    }

    #[must_use] pub fn lookup(&self, key: usize) -> &V {
        assert!(key < self.storage.len());
        &self.storage[key]
    }

    pub fn lookup_mut(&mut self, key: usize) -> &mut V {
        assert!(key < self.storage.len());

        &mut self.storage[key]
    }

    #[must_use] pub fn len(&self) -> usize {
        self.uniq.len()
    }
}

impl<V: Hash + Eq> IntoIterator for Interner<V> {
    type Item = V;

    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.storage.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::Interner;

    #[test]
    fn check_interning() {
        let mut interner = Interner::default();
        let key = interner.intern("foo");

        assert_eq!(key, interner.intern("foo"));
        assert_eq!(key, interner.intern("foo"));
        assert_eq!(key, interner.intern("foo"));
    }

    #[test]
    fn lookup() {
        let mut interner = Interner::default();

        interner.intern("foo");
        let key = interner.intern("bar");
        interner.intern("baz");

        assert_eq!(&"bar", interner.lookup(key));
    }
}
