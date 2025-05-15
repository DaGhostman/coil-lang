use std::{hash::Hash, marker::PhantomData};

use crate::calculate_hash;

const LOAD_FACTOR: f64 = 0.75;

pub struct Entry<V> {
    key: usize,
    value: V,
}

impl<V> Entry<V> {
    pub fn new(hash: usize, value: V) -> Self {
        Self { key: hash, value }
    }

    pub fn is_empty(&self) -> bool {
        self.key == 0
    }

    pub fn key(&self) -> usize {
        self.key
    }

    pub fn value(&self) -> &V {
        &self.value
    }
}

pub struct Table<K, V>
where
    K: Hash,
{
    count: usize,
    capacity: usize,
    entries: Vec<Entry<V>>,
    _phantom: PhantomData<K>,
}

impl<K, V> Default for Table<K, V>
where
    K: Hash,
    V: Default,
 {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Table<K, V>
where
    K: Hash,
    V: Default,
{
    #[must_use] pub fn new() -> Self {
        let mut entries = Vec::with_capacity(32);
        entries.resize_with(32, || Entry::new(0, V::default()));

        Table::<K, V> {
            count: 0,
            capacity: 32,
            entries,
            _phantom: PhantomData,
        }
    }

    pub fn grow(&mut self) {
        self.entries
            .resize_with(self.capacity + 16, || Entry::new(0, V::default()));
        self.capacity += 16;
    }

    pub fn insert(&mut self, key: K, value: V) -> bool {
        let hash = calculate_hash(&key) as usize;

        if self.count + 1 > (self.capacity as f64 * LOAD_FACTOR) as usize {
            self.grow();
        }

        let mut is_new = true;
        if let Some(entry) = self.find_mut(hash) {
            is_new = entry.is_empty();

            *entry = Entry::new(hash, value);
        }

        if is_new {
            self.count += 1;
        }

        is_new
    }

    pub fn get(&self, key: K) -> Option<&V> {
        if self.count == 0 {
            return None;
        }

        let hash = calculate_hash(&key) as usize;

        if let Some(entry) = self.find(hash) {
            if entry.is_empty() {
                return None;
            }

            return Some(entry.value());
        }

        None
    }

    pub fn remove(&mut self, key: K) -> bool {
        let hash = calculate_hash(&key) as usize;

        if let Some(entry) = self.find_mut(hash) {
            *entry = Entry::new(0, V::default());
            self.count -= 1;

            return true;
        }

        false
    }

    fn find(&self, key: usize) -> Option<&Entry<V>> {
        let mut idx = key % self.capacity;

        while idx < self.capacity {
            if let Some(entry) = self.entries.get(idx) {
                if entry.is_empty() || key == entry.key() {
                    return self.entries.get(idx);
                }
            }

            idx = (idx + 1) % self.capacity;
        }

        None
    }

    fn find_mut(&mut self, key: usize) -> Option<&mut Entry<V>> {
        let mut idx = key % self.capacity;

        while idx < self.capacity {
            if let Some(entry) = self.entries.get(idx) {
                if entry.is_empty() || key == entry.key() {
                    return self.entries.get_mut(idx);
                }
            }

            idx = (idx + 1) % self.capacity;
        }

        None
    }
}
