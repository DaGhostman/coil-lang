use std::{ops::Index, vec::IntoIter};

#[derive(Clone, Debug)]
pub struct VecArray<T, const N: usize>
where
    T: Clone,
{
    size: usize,
    storage: [T; N],
    expansions: Vec<[T; N]>,
}

fn get_slot_index(index: usize, max: usize) -> (usize, usize) {
    let slot = (index / max) - 1;
    (slot, index % max)
}

impl<T, const N: usize> Default for VecArray<T, N>
where
    T: Default + Clone + Copy,
{
    fn default() -> Self {
        Self {
            size: 0,
            storage: [T::default(); N],
            expansions: vec![],
        }
    }
}

impl<T, const N: usize> VecArray<T, N>
where
    T: Default + Clone + Copy,
{
    pub fn contains_key(&self, index: usize) -> bool {
        self.size > index
    }

    pub fn get(&self, index: usize) -> &T {
        if index < N {
            &self.storage[index]
        } else {
            let (slot, idx) = get_slot_index(index, N);

            &self.expansions[slot][idx]
        }
    }

    pub fn get_mut(&mut self, index: usize) -> &mut T {
        if index < N {
            &mut self.storage[index]
        } else {
            let (slot, idx) = get_slot_index(index, N);
            &mut self.expansions[slot][idx]
        }
    }
    pub fn push(&mut self, value: T) {
        self.insert(self.size, value);
    }
    pub fn insert(&mut self, index: usize, value: T) {
        if self.size <= index {
            self.size = self.size.max(index) + 1;
        }

        if index < N {
            self.storage[index] = value;
        } else {
            self.grow(index);
            let (slot, index) = get_slot_index(index, N);

            self.expansions[slot][index] = value;
        }
    }

    pub fn grow(&mut self, index: usize) {
        if index < N {
            return;
        }

        if index >= self.size - 1 {
            let (slot, _) = get_slot_index(index, N);
            if self.expansions.len() <= slot {
                self.expansions.resize(slot + 1, [T::default(); N]);
            }
        }
    }

    pub fn modify_or_insert<F: FnOnce(&mut T)>(&mut self, index: usize, or: F) {
        if self.size > index {
            or(if index < N {
                &mut self.storage[index]
            } else {
                let (slot, idx) = get_slot_index(index, N);
                &mut self.expansions[slot][idx]
            });
        } else {
            self.insert(index, Default::default());
        }
    }

    pub fn clear(&mut self) {
        self.storage = [T::default(); N];
        self.expansions.clear();
        self.size = 0;
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn slots(&self) -> usize {
        1 + self.expansions.len()
    }

    pub fn capacity(&self) -> usize {
        self.slots() * N
    }
}

impl<T, const N: usize> IntoIterator for VecArray<T, N>
where
    T: Clone,
{
    type Item = T;

    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        let mut iterable = self.storage.to_vec();
        iterable.reserve(self.expansions.len() * (N - 1));
        for slot in self.expansions {
            iterable.extend(slot);
        }

        iterable.into_iter()
    }
}

impl<T, const N: usize> Index<usize> for VecArray<T, N>
where
    T: Clone + Copy + Default,
{
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
    }
}

impl<T, const N: usize> Into<Vec<T>> for VecArray<T, N>
where
    T: Clone + Copy,
{
    fn into(self) -> Vec<T> {
        let mut result = Vec::with_capacity(self.size);
        result.copy_from_slice(&self.storage);
        for slice in self.expansions {
            result.copy_from_slice(&slice);
        }

        result
    }
}

#[cfg(test)]
mod test {
    use super::VecArray;

    #[test]
    pub fn test_slots_assertion() {
        let mut arr: VecArray<i32, 32> = VecArray::default();

        for i in 0..=64 {
            arr.push(i);
        }

        for i in 0..=64 {
            assert_eq!(i, *arr.get(i as usize));
        }
    }
}
