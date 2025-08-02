use common::{promise, ArrayVec};

pub struct Stack<T: Default + Copy + Clone, const N: usize> {
    top: usize,
    storage: ArrayVec<T, N>,
}

impl <T: Default + Copy + Clone, const N: usize> Default for Stack<T, N> {
    fn default() -> Self {
        Self {
            top: 0,
            storage: ArrayVec::default(),
        }
    }
}

impl <T: Default + Copy + Clone + PartialEq, const N: usize> Stack<T, N> {
    pub fn push(&mut self, value: T) -> () {
        self.storage[self.top] = value;
        self.top += 1;
    }

    pub fn pop(&mut self) -> &T {
        promise!(self.top > 0);

        self.top -= 1;
        &self.storage[self.top]
    }

    pub fn peek(&self, idx: usize) -> &T {
        promise!(self.top >= idx);

        &self.storage[self.top - idx]
    }

    pub fn seek(&mut self, top: usize) -> () {
        self.top = top;
    }

    pub fn tell(&self) -> usize {
        self.top
    }
}

