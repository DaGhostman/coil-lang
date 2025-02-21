use std::fmt::{Debug, Display};

use ahash::{HashMap, HashMapExt};

use super::ref_counter::RefCounter;

#[derive(PartialEq, Eq, Hash, Default, Copy, Clone)]
pub struct Key {
    slot: usize,
    version: u32,
}

impl Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({};{})", self.slot, self.version)
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({};{})", self.slot, self.version)
    }
}

impl Key {
    pub(crate) fn new(slot: usize, version: u32) -> Self {
        Self { slot, version }
    }
}

pub struct Arena<T> {
    cursor: usize,
    slots: HashMap<usize, T>,
    free_slots: Vec<usize>,
    versions: Vec<u32>,
    growth_factor: usize,

    counter: RefCounter<Key>,
}

impl<T> Arena<T> {
    pub fn with_capacity(capacity: usize, growth_scale: Option<usize>) -> Self {
        let mut versions = Vec::with_capacity(capacity);
        versions.fill(0);

        Self {
            cursor: 0,
            slots: HashMap::with_capacity(capacity),
            free_slots: Vec::with_capacity(capacity),
            versions,
            growth_factor: (capacity + (capacity % 8))
                / if let Some(scale) = growth_scale {
                    scale
                } else {
                    4
                },
            counter: RefCounter::default(),
        }
    }

    pub fn grow(&mut self) {
        self.slots.reserve(self.growth_factor);
        self.versions.reserve(self.growth_factor);
        self.versions.resize(self.slots.capacity(), 0);
    }

    pub fn shrink(&mut self) {
        let len = self.slots.len() + (self.slots.len() % 8);
        if self.slots.len() < len {
            self.slots.shrink_to(len);
            self.free_slots.shrink_to(len);
            self.versions.shrink_to(len);
        }
    }

    pub fn alloc(&mut self, value: T) -> Key {
        if self.is_loaded(0.75) {
            self.counter.collect().iter().for_each(|key| {
                if !self.free_slots.contains(&key.slot) {
                    self.free(*key);
                }
            });
        }

        if self.free_slots.is_empty() {
            self.grow();
        }

        let slot = if let Some(slot) = self.free_slots.pop() {
            slot
        } else {
            let slot = self.cursor;
            self.cursor += 1;

            slot
        };

        self.slots.insert(slot, value);
        let key = Key::new(slot, self.versions[slot]);

        self.counter.alloc(key);

        key
    }

    pub fn get(&self, index: Key) -> Option<&T> {
        if !self.is_latest(index) {
            return None;
        }

        self.slots.get(&index.slot)
    }

    pub fn get_mut(&mut self, index: Key) -> Option<&mut T> {
        if !self.is_latest(index) {
            return None;
        }

        self.slots.get_mut(&index.slot)
    }

    pub fn is_latest(&self, index: Key) -> bool {
        if let Some(version) = self.versions.get(index.slot) {
            *version == index.version
        } else {
            false
        }
    }

    pub fn free(&mut self, index: Key) {
        self.versions[index.slot] += 1;
        self.free_slots.push(index.slot);
        self.slots.remove(&index.slot);

        self.dereference(index);
        self.shrink();
    }

    pub fn reference(&mut self, key: Key) {
        self.counter.increase(key);
    }

    pub fn dereference(&mut self, key: Key) {
        self.counter.decrease(key);
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn capacity(&self) -> usize {
        self.slots.capacity()
    }

    pub fn is_loaded(&self, hwm: f64) -> bool {
        (self.slots.len() as f64 / self.slots.capacity() as f64) >= hwm
    }
}

impl<T> Drop for Arena<T> {
    fn drop(&mut self) {
        self.slots.drain();
    }
}

#[cfg(test)]
mod tests {
    use crate::memory::arena::Key;

    use super::Arena;

    #[test]
    fn test_simple_allocation() {
        let mut arena = Arena::with_capacity(14, None);
        assert_eq!(arena.len(), 0);
        assert_eq!(arena.capacity(), 14);
        arena.alloc(42);
        assert_eq!(arena.len(), 1);
        assert_eq!(arena.capacity(), 14);
    }

    #[test]
    fn test_key_props() {
        let mut arena = Arena::with_capacity(14, None);
        arena.alloc(0);
        arena.alloc(0);
        arena.alloc(0);
        arena.alloc(0);
        arena.alloc(0);

        assert_eq!(arena.len(), 5);
        assert_eq!(arena.capacity(), 14);

        let key = arena.alloc(42);
        assert_eq!(key, Key::new(5, 0));
    }

    #[test]
    fn test_versioning() {
        let mut arena = Arena::with_capacity(14, None);
        let key = arena.alloc(0);
        assert_eq!(arena.capacity(), 14);
        assert_eq!(arena.len(), 1);
        arena.free(key);
        assert_eq!(arena.is_latest(key), false);

        let key = arena.alloc(42);
        assert_eq!(arena.capacity(), 14);
        assert_eq!(arena.len(), 1);
        assert_eq!(1, key.version);
        assert_eq!(0, key.slot);
    }
}
