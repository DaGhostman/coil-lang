use rustc_hash::{FxBuildHasher, FxHashMap as HashMap, FxHasher};
use std::hash::{Hash, Hasher};

fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interner<V>
where
    V: Hash + Clone + Eq,
{
    uniq: HashMap<u64, usize>,
    storage: Vec<V>,
}

impl<V: Eq + Hash + Clone> Default for Interner<V> {
    fn default() -> Self {
        Interner {
            uniq: HashMap::with_capacity_and_hasher(32, FxBuildHasher::default()),
            storage: Vec::with_capacity(32),
        }
    }
}

impl<V: Hash + Clone + Eq + std::fmt::Debug> Interner<V> {
    pub fn intern(&mut self, value: V) -> usize {
        let hash = calculate_hash(&value);

        return *self.uniq.entry(hash).or_insert_with(|| {
            self.storage.push(value);

            self.storage.len() - 1
        });
    }

    pub fn lookup(&self, key: usize) -> Option<&V> {
        self.storage.get(key)
    }

    pub fn lookup_mut(&mut self, key: usize) -> Option<&mut V> {
        self.storage.get_mut(key)
    }

    pub fn len(&self) -> usize {
        self.uniq.len()
    }

    pub fn is_empty(&self) -> bool {
        self.uniq.is_empty()
    }
}

impl<V: Hash + Clone + Eq> IntoIterator for Interner<V> {
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

        assert_eq!(Some(&"bar"), interner.lookup(key));
    }
}
