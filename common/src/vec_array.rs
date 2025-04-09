use std::ops::Index;

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
    (index / max, index % max)
}

impl<T, const N: usize> Default for VecArray<T, N>
where
    T: Default + Clone,
{
    fn default() -> Self {
        Self {
            size: 0,
            storage: core::array::from_fn::<T, N, _>(|_| Default::default()),
            expansions: vec![],
        }
    }
}

impl<T, const N: usize> VecArray<T, N>
where
    T: Default + Clone,
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
            self.storage[index] = value
        } else {
            let (slot, index) = get_slot_index(index, N);

            self.expansions[slot][index] = value;
        }
    }

    pub fn grow(&mut self, items: usize) {
        if items >= self.size {
            let (slot, _) = get_slot_index(self.size, N);
            if self.expansions.len() <= slot {
                self.expansions.resize(
                    slot + 1,
                    core::array::from_fn::<T, N, _>(|_| Default::default()),
                );
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
        self.storage = core::array::from_fn::<T, N, _>(|_| Default::default());
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

impl<T, const N: usize> Index<usize> for VecArray<T, N>
where
    T: Clone + Default,
{
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
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
