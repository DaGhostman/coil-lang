use std::{ops::{Index, IndexMut}, slice::{Iter, IterMut}};

use crate::{likely, promise, unlikely};

pub struct ArrayVec<T: Copy + Default , const N: usize> {
    current: usize,
    storage: [T; N],
    expansion: Vec<T>,
}

impl <T: Copy + Default, const N: usize>Default for ArrayVec<T, N> {
    fn default() -> Self {
        Self {
            current: 0,
            storage: [T::default(); N],
            expansion: Vec::default(),
        }
    }
}

impl <T: Copy + Default, const N: usize> ArrayVec<T, N> {
    fn grow(&mut self, cursor: usize) {
        promise!(cursor >= N);
        let boundary = self.expansion.len();

        if unlikely(cursor >= boundary) {
            self.expansion.resize(boundary + N, T::default());
        }
    }

    pub fn current(&self) -> &T {
        if likely(self.current < N) {
            promise!(self.current < N);

            &self.storage[self.current]
        } else {
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

            promise!(index >= N);
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

            self.storage[self.current] = value;
        } else {
            self.grow(self.current - N);

            promise!(self.current > N);
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


impl <T: Copy + Default, const N: usize> Index<usize> for ArrayVec<T, N> {
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

impl <T: Copy + Default, const N: usize> IndexMut<usize> for ArrayVec<T, N> {
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
