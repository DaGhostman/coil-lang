use rustc_hash::{FxBuildHasher, FxhashMap as HashMap};
use std::fmt::Debug;

use super::collector::{Collector, Gc};

#[derive(Default)]
pub struct Heap<T>
where
    T: Debug,
{
    storage: HashMap<usize, Gc<T>>,
    slots: Vec<usize>,
}

impl<T> Heap<T>
where
    T: Debug,
{
    pub fn with_capacity(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        let mut cursor = 0;
        slots.fill_with(|| {
            cursor += 1;

            cursor - 1
        });

        Self {
            storage: HashMap::with_capacity_and_hasher(capacity, FxBuildHasher::default()),
            slots,
        }
    }

    pub fn alloc(&mut self, value: T) -> usize {
        let index = if !self.slots.is_empty() {
            self.slots.pop().unwrap()
        } else {
            self.grow();

            self.len()
        };

        if let Some(val) = self.storage.get_mut(index) {
            *val = Box::from(value);
        } else {
            self.storage.push(Box::from(value));
        }

        index
    }

    pub fn lookup(&self, index: usize) -> Option<&Box<T>> {
        if self.slots.contains(&index) {
            None
        } else {
            self.storage.get(index)
        }
    }

    pub fn dealloc(&mut self, index: &usize) {
        if !self.slots.contains(index) {
            self.slots.push(*index);
        }
    }

    pub fn free(&mut self, index: &usize) {
        self.dealloc(index);

        drop(self.storage.remove(*index))
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn capacity(&self) -> usize {
        self.storage.capacity()
    }

    fn grow(&mut self) {
        let capacity = self.storage.capacity() as f64;
        let size = (self.storage.len() - self.slots.len()) as f64;

        let ratio = size / capacity;

        if ratio > 0.75 {
            self.storage.reserve(8.max((ratio / 2.0) as usize));
        }
    }

    fn should_collect(&self) -> bool {
        let unused = self.collector.get_unused();
        let ratio = unused.len().max(1) as f64 / self.len().max(1) as f64; // / unused.len().max(1) as f64;

        dbg!(ratio);

        ratio < 0.75
    }

    pub fn collect(&mut self) {
        if self.should_collect() {
            self.collector.get_unused().iter().for_each(|value| {
                self.free(value);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Heap;

    #[test]
    fn test_simple_allocation() {
        let mut heap = Heap::with_capacity(1);
        let index = heap.alloc(69);
        assert_eq!(0, index);
        assert_eq!(heap.lookup(index).map(|val| **val), Some(69));
        assert_eq!(heap.len(), 1);
        heap.dealloc(&index);
        let index = heap.alloc(420);
        assert_eq!(0, index);
        assert_eq!(heap.lookup(index).map(|val| **val), Some(420));
        assert_eq!(heap.len(), 1);
        heap.dealloc(&index);
        let index = heap.alloc(42);
        assert_eq!(0, index);
        assert_eq!(heap.lookup(index).map(|val| **val), Some(42));
        assert_eq!(heap.len(), 1);
        heap.free(&index);
        assert_eq!(heap.len(), 0);
    }
}
