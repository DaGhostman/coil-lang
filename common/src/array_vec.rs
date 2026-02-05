use std::ops::{Index, IndexMut};

use crate::{likely, promise, unlikely};

#[derive(Clone)]
pub struct ArrayVec<T: Default, const N: usize> {
    current: usize,
    storage: [T; N],
    expansion: Vec<T>,
}

impl<T: Default, const N: usize> FromIterator<T> for ArrayVec<T, N> {
    fn from_iter<X: IntoIterator<Item = T>>(iter: X) -> Self {
        let mut result = ArrayVec::default();
        iter.into_iter().for_each(|v| result.push(v));

        result
    }
}

impl<T: Default, const N: usize> Default for ArrayVec<T, N> {
    fn default() -> Self {
        Self {
            current: 0,
            storage: std::array::from_fn(|_| T::default()),
            expansion: Vec::default(),
        }
    }
}

impl<T: Default, const N: usize> ArrayVec<T, N> {
    pub fn iter<'iter>(&'iter self) -> ArrayVecIter<'iter, T, N> {
        ArrayVecIter::new(self)
    }

    fn grow(&mut self, cursor: usize) {
        let boundary = self.expansion.len();

        if unlikely(cursor >= boundary) {
            self.expansion.resize_with(boundary + N, T::default);
        }
    }

    #[inline]
    pub fn current(&self) -> &T {
        if likely(self.current < N) {
            promise!(self.current < N);

            &self.storage[self.current]
        } else {
            promise!((self.current - N) < self.expansion.len());
            &self.expansion[self.current - N]
        }
    }

    #[inline]
    pub fn current_mut(&mut self) -> &mut T {
        if likely(self.current < N) {
            promise!(self.current < N);

            &mut self.storage[self.current]
        } else {
            let index = self.current - N;
            self.grow(index);

            promise!(index < self.expansion.len());

            &mut self.expansion[index]
        }
    }

    #[inline]
    pub fn consume(&mut self) {
        self.current += 1;
    }

    #[inline]
    pub fn seek(&mut self, value: usize) {
        self.current = value;
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        let current = self.current;
        self.current += 1;

        if likely(current < N) {
            promise!(current < N);
            promise!(current < self.storage.len());

            self.storage[current] = value;
        } else {
            let offset = current - N;
            self.grow(offset);

            promise!(offset >= N);
            promise!(offset < self.expansion.len());

            self.expansion[offset] = value;
        }
    }

    #[inline]
    pub fn pop(&mut self) -> &T {
        promise!(self.current > 0);
        promise!(!self.storage.is_empty() || !self.expansion.is_empty());

        self.current -= 1;
        if likely(self.current < N) {
            &self.storage[self.current]
        } else {
            promise!(self.current >= N);
            promise!(self.current - N < self.expansion.len());
            &self.expansion[self.current - N]
        }
    }

    pub fn get(&self) -> &T {
        promise!(self.current > 0);
        let current = self.current - 1;

        if likely(current < N) {
            promise!(current < N);
            promise!(current < self.storage.len());

            &self.storage[current]
        } else {
            promise!(self.expansion.len() > current - N);
            &self.expansion[current - N]
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        promise!(self.current > 0);
        let current = self.current - 1;

        if likely(current < N) {
            promise!(current < N);
            &mut self.storage[current]
        } else {
            self.grow(current - N);

            promise!(self.expansion.len() > current - N);
            &mut self.expansion[current - N]
        }
    }

    // pub fn insert(&mut self, index: usize, value: T) {
    //     self.current = self.current.max(index + 1);
    //
    //     if likely(index < N) {
    //         promise!(index < N);
    //         self.storage[index] = value;
    //     } else {
    //         self.grow(index - N);
    //         promise!(index - N < self.expansion.len());
    //         self.expansion[index - N] = value;
    //     }
    // }

    #[inline]
    pub fn len(&self) -> usize {
        self.current
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.current == 0
    }

    // pub fn iter(&self) -> Iter<T>{
    //     promise!(self.current < N);
    //
    //     self.storage[0..self.current].iter()
    // }
    //
    // pub fn iter_mut(&mut self) -> IterMut<T> {
    //     promise!(self.current < N);
    //
    //     self.storage[0..self.current].iter_mut()
    // }
    //
    // pub fn drain(&mut self) -> &[T] {
    //     let cursor = self.current;
    //     self.current = 0;
    //
    //     &self.storage[0..cursor]
    // }

    #[inline]
    pub fn clear(&mut self) {
        self.current = 0;
        // self.expansion.clear();
    }
}

impl<T: Default, const N: usize> Index<usize> for ArrayVec<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        if likely(index < N) {
            promise!(index < N);
            &self.storage[index]
        } else {
            promise!(index - N < self.expansion.len());
            &self.expansion[index - N]
        }
    }
}

// impl<T: Default + Copy, const N: usize> Index<Range<usize>>for ArrayVec<T, N> {
//     type Output = ArrayVec<T, 16>;
//     fn index(&self, index: Range<usize>) -> Self::Output {
//         let mut v = ArrayVec::<T, 16>::default();
//
//         for n in index.start..index.end {
//             v.push(self[n]);
//         }
//
//         v
//
//         // if (index < N) {
//         //     promise!(index < N);
//         //     &self.storage[index]
//         // } else {
//         //     promise!(index - N < self.expansion.len());
//         //     &self.expansion[index - N]
//         // }
//     }
// }
//
impl<T: Default, const N: usize> IndexMut<usize> for ArrayVec<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.current = self.current.max(index + 1);
        if likely(index < N) {
            promise!(index < N);
            &mut self.storage[index]
        } else {
            self.grow(index - N);
            promise!(index - N < self.expansion.len());
            &mut self.expansion[index - N]
        }
    }
}

#[cfg(debug_assertions)]
impl<T: Default + std::fmt::Debug, const N: usize> std::fmt::Debug for ArrayVec<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}",
            self.storage
                .iter()
                .chain(self.expansion.iter())
                .enumerate()
                .filter_map(|(idx, val)| {
                    if idx >= self.current {
                        return None;
                    }

                    Some(val.to_owned())
                })
                .collect::<Vec<_>>()
        )
    }
}

pub struct ArrayVecIter<'iter, T: Default, const N: usize> {
    cursor: usize,
    value: &'iter ArrayVec<T, N>,
}

impl<'iter, T: Default, const N: usize> ArrayVecIter<'iter, T, N> {
    pub fn seek(&mut self, cursor: usize) {
        self.cursor = cursor;
    }
}

impl<'iter, T: Default, const N: usize> ArrayVecIter<'iter, T, N> {
    pub fn new(value: &'iter ArrayVec<T, N>) -> Self {
        Self { cursor: 0, value }
    }
}

impl<'iter, T: Default, const N: usize> Iterator for ArrayVecIter<'iter, T, N> {
    type Item = &'iter T;
    fn next(&mut self) -> Option<Self::Item> {
        let mut value = None;

        if likely(self.cursor < self.value.len()) {
            promise!(self.cursor < self.value.len());
            value = Some(&self.value[self.cursor]);
        }

        self.cursor += 1;

        value
    }
}
