use std::{
    fmt::Debug,
    iter::Chain,
    ops::{Index, IndexMut},
    slice::Iter,
    vec::IntoIter as VecIntoIter,
};

use crate::{likely, promise, unlikely};

#[derive(Clone)]
pub struct ArrayVec<T: Default, const N: usize> {
    current: usize,
    storage: [T; N],
    expansion: Vec<T>,
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
        ArrayVecIter::new(&self)
    }

    fn grow(&mut self, cursor: usize) {
        let boundary = self.expansion.len();

        if unlikely(cursor >= boundary) {
            self.expansion.resize_with(boundary + N, T::default);
        }
    }

    pub fn current(&self) -> &T {
        if likely(self.current < N) {
            promise!(self.current < N);

            &self.storage[self.current]
        } else {
            promise!((self.current - N) < self.expansion.len());
            &self.expansion[self.current - N]
        }
    }

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

    pub fn consume(&mut self) -> () {
        self.current += 1;
    }

    pub fn seek(&mut self, value: usize) -> () {
        self.current = value;
    }

    pub fn push(&mut self, value: T) -> () {
        if likely(self.current < N) {
            promise!(self.current < N);
            promise!(self.current < self.storage.len());

            self.storage[self.current] = value;
        } else {
            self.grow(self.current - N);

            promise!(self.current >= N);
            promise!(self.current - N < self.expansion.len());

            self.expansion[self.current - N] = value;
        }

        self.current += 1;
    }

    pub fn pop(&mut self) -> &T {
        promise!(self.current > 0);

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

    pub fn len(&self) -> usize {
        self.current
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

    pub fn clear(&mut self) {
        self.current = 0;
    }
}

impl<T: Default, const N: usize> Index<usize> for ArrayVec<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        if likely(index < N) {
            promise!(index < N);
            &self.storage[index]
        } else {
            promise!(index - N > self.expansion.len());
            &self.expansion[index - N]
        }
    }
}

impl<T: Default, const N: usize> IndexMut<usize> for ArrayVec<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        if likely(index < N) {
            promise!(index < N);
            &mut self.storage[index]
        } else {
            self.grow(index - N);
            promise!(index - N > self.expansion.len());
            &mut self.expansion[index - N]
        }
    }
}

#[cfg(debug_assertions)]
impl<T: Default + Debug, const N: usize> Debug for ArrayVec<T, N> {
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
