use std::hash::Hash;

use ahash::{HashMap, HashMapExt};

#[derive(Clone, Debug, PartialEq)]
pub struct Interner<V>
where
    V: Hash + Clone + Eq,
{
    cursor: usize,
    uniq: HashMap<V, usize>,
    storage: Vec<V>,
}

impl<V: Eq + Hash + Clone> Default for Interner<V> {
    fn default() -> Self {
        Interner {
            cursor: 0,
            uniq: HashMap::with_capacity(32),
            storage: Vec::with_capacity(32),
        }
    }
}

impl<V: Eq + Hash + Clone> From<Vec<V>> for Interner<V> {
    fn from(collection: Vec<V>) -> Self {
        let mut interner = Self::default();
        for (idx, val) in collection.iter().enumerate() {
            let i = interner.intern(val.to_owned());

            if i != idx {
                panic!("Key mismatch");
            }
        }

        interner
    }
}

impl<V: Hash + Clone + Eq> Interner<V> {
    pub fn intern(&mut self, value: V) -> usize {
        let hash = self.cursor;

        if self.uniq.contains_key(&value) {
            return self.uniq.get(&value).copied().unwrap();
        }

        self.uniq.insert(value.clone(), hash);
        self.storage.push(value);
        self.cursor += 1;

        hash
    }

    pub fn lookup(&self, key: usize) -> Option<&V> {
        self.storage.get(key)
    }

    pub fn len(&self) -> usize {
        self.uniq.len()
    }

    pub fn is_empty(&self) -> bool {
        self.uniq.is_empty()
    }

    pub fn dump(&self) -> Vec<V> {
        self.storage.iter().map(|value| value.to_owned()).collect()
    }

    pub fn import(&mut self, items: Vec<V>) {
        let size = items.len();
        self.storage.reserve(size);
        self.uniq.reserve(size);

        for value in items {
            self.intern(value.to_owned());
        }
        self.cursor = size;
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
