use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    hash::Hash,
};

#[derive(Default)]
pub struct RefCounter<K>
where
    K: Hash + Eq + Copy,
{
    storage: HashMap<K, usize>,

    relations: HashMap<K, HashSet<K>>,
}

impl<K> RefCounter<K>
where
    K: Hash + Eq + Copy + Display + Debug,
{
    pub fn alloc(&mut self, key: K) {
        self.storage.insert(key, 0);
        self.relations.insert(key, HashSet::with_capacity(32));
    }

    pub fn increase(&mut self, key: K) {
        self.storage.entry(key).and_modify(|item| {
            *item += 1;
        });
    }

    pub fn decrease(&mut self, key: K) {
        self.storage.entry(key).and_modify(|item| {
            if let Some(n) = item.checked_sub(1) {
                *item = n;
            } else {
                // unreachable!("Attempting do decrease 0");
            }
        });
    }

    pub fn references(&mut self, owner: K, reference: K) {
        self.relations.entry(owner).and_modify(|item| {
            item.insert(reference);
        });
    }

    pub fn is_referenced(&mut self, key: K) -> bool {
        self.storage.get(&key).is_some()
    }

    pub fn collect(&mut self) -> Vec<K> {
        let mut keys = vec![];
        self.storage
            .clone()
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(key, _)| key)
            .for_each(|key| {
                self.storage.remove(key);
                keys.push(*key);

                if let Some(rels) = self.relations.get(&key).cloned() {
                    rels.iter().copied().for_each(|key| self.decrease(key));
                }
            });

        keys
    }
}
