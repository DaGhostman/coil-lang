use std::ops::{Index, IndexMut};

use crate::likely;

#[derive(Clone)]
pub struct VecArray<T, const N: usize>
where
    T: Clone,
{
    size: usize,
    storage: [T; N],
    expansions: Vec<[T; N]>,
}

fn get_slot_index(index: usize, max: usize) -> (usize, usize) {
    let slot = (index / max);

    (slot, index % max)
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
    #[inline]
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

    pub fn push(&mut self, value: T) -> usize {
        let size = self.size;

        if size < N {
            self.storage[size] = value;
        } else {
            let (slot, index) = get_slot_index(size, N);
            if self.expansions.len() == slot {
                let (max_slot, _) = get_slot_index(size, N);
                self.expansions.resize(
                    max_slot + 4,
                    core::array::from_fn::<T, N, _>(|_| Default::default()),
                );
            }

            self.expansions[slot][index] = value;
        }

        self.size += 1;

        size
    }

    pub fn insert(&mut self, index: usize, value: T) {
        if index < N {
            self.storage[index] = value;
        } else {
            self.size = self.size.max(index) + 1;

            let (slot, index) = get_slot_index(index, N);
            if self.expansions.len() <= slot {
                let (max_slot, _) = get_slot_index(index, N);
                self.expansions.resize(
                    max_slot + 4,
                    core::array::from_fn::<T, N, _>(|_| Default::default()),
                );
            }

            self.expansions[slot][index] = value;
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }
}

// impl<T, const N: usize> IndexMut<usize> for VecArray<T, N>
// where
//     T: Clone + Default,
// {
//     fn index_mut(&mut self, index: usize) -> &mut Self::Output {
//         if index < N {
//             likely(true);
//             &mut self.storage[index]
//         } else {
//             let (slot, idx) = get_slot_index(index, N);
//             if !self.contains_key(index) {
//                 self.insert(index, Default::default());
//             }
//
//             &mut self.expansions[slot][idx]
//         }
//     }
// }
//
// impl<T, const N: usize> Index<usize> for VecArray<T, N>
// where
//     T: Clone + Default,
// {
//     type Output = T;
//
//     fn index(&self, index: usize) -> &Self::Output {
//         if index < N {
//             likely(true);
//             &self.storage[index]
//         } else {
//             let (slot, index) = get_slot_index(index, N);
//             &self.expansions[slot][index]
//         }
//     }
// }

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
