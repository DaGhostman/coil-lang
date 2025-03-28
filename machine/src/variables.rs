use std::hash::Hash;

use rustc_hash::FxHashMap as HashMap;

#[derive(Eq, PartialEq)]
pub struct Key {
    name: usize,
    pub(crate) scope: usize,
}

impl Key {
    pub fn new(name: usize, scope: usize) -> Self {
        Self { name, scope }
    }
}

impl Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.name);
        state.write_usize(self.scope);
    }
}

#[derive(Default)]
pub struct Variables {
    storage: HashMap<Key, usize>,
}

impl Variables {
    pub fn lookup(&self, variable: Key) -> usize {
        self.storage[&variable]
    }

    pub fn insert(&mut self, variable: Key, value: usize) {
        self.storage.insert(variable, value);
    }
}
