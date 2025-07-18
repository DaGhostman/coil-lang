use std::{ops::{Index, IndexMut}, slice::{Iter, IterMut}};

use crate::promise;

#[derive(Copy, Clone)]
pub struct ArrayVec<T: Copy + Default , const N: usize> {
    current: usize,
    storage: [T; N],
}

impl <T: Copy + Default, const N: usize>Default for ArrayVec<T, N> {
    fn default() -> Self {
        Self {
            current: 0,
            storage: [T::default(); N],
        }
    }
}

impl <T: Copy + Default, const N: usize> ArrayVec<T, N> {

    pub fn current(&self) -> &T {
        &self.storage[self.current]
    }

    pub fn current_mut(&mut self) -> &mut T {
        &mut self.storage[self.current]
    }
    
    pub fn consume(&mut self) -> () {
        self.current += 1;
    }

    pub fn seek(&mut self, value: usize) -> () {
        self.current = value;
    }

    pub fn push(&mut self, value: T) -> () {
        promise!(self.current < N);

        self.storage[self.current] = value;
        self.current += 1;
    }

    pub fn pop(&mut self) -> &T {
        promise!(self.current > 0);
        self.current -= 1;
        &self.storage[self.current + 1]
    }

    pub fn get(&self) -> &T {
        promise!(self.current > 0);

        &self.storage[self.current - 1]
    }

    pub fn get_mut(&mut self) -> &mut T {
        promise!(self.current > 0);

        &mut self.storage[self.current - 1]
    }

    pub fn len(&self) -> usize {
        self.current 
    }

    pub fn iter(&self) -> Iter<T>{
        promise!(self.current < N);

        self.storage[0..self.current].iter()
    }

    pub fn iter_mut(&mut self) -> IterMut<T> {
        promise!(self.current < N);

        self.storage[0..self.current].iter_mut()
    }

    pub fn drain(&mut self) -> &[T] {
        let cursor = self.current;
        self.current = 0;

        &self.storage[0..cursor]
    }

    pub fn clear(&mut self) {
        self.current = 0;
    }
}


impl <T: Copy + Default, const N: usize> Index<usize> for ArrayVec<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        &self.storage[index]
    }
}

impl <T: Copy + Default, const N: usize> IndexMut<usize> for ArrayVec<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.storage[index]
    }
}
