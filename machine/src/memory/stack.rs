use std::ops::{Index, IndexMut, Range};

use common::promise;

pub struct Stack<T: Default, const N: usize> {
    stack: [T; N],
    cursor: usize,
}

impl<T: Default + Copy, const N: usize> Default for Stack<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default + Copy, const N: usize> Stack<T, N> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: std::array::from_fn(|_| T::default()),
            cursor: 0,
        }
    }

    #[inline]
    pub fn pop(&mut self) -> T {
        promise!(self.cursor > 0);
        self.cursor -= 1;
        self.stack[self.cursor]
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        promise!(self.cursor < N);
        self.stack[self.cursor] = value;
        self.cursor += 1;
    }

    #[inline]
    pub fn peek(&self) -> &T {
        promise!(self.cursor > 0);
        &self.stack[self.cursor - 1]
    }

    #[inline]
    pub fn seek(&mut self, idx: usize) {
        promise!(idx <= N);
        self.cursor = idx;
    }

    #[inline]
    pub fn top(&mut self) -> &mut T {
        promise!(self.cursor > 0);
        &mut self.stack[self.cursor - 1]
    }

    #[inline]
    pub fn tell(&self) -> usize {
        self.cursor
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.stack[..self.cursor]
    }
}

impl<T: Default, const N: usize> Index<Range<usize>> for Stack<T, N> {
    type Output = [T];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        promise!(index.end <= N);
        &self.stack[index]
    }
}

impl<T: Default + Copy, const N: usize> IntoIterator for Stack<T, N> {
    type Item = T;
    type IntoIter = std::array::IntoIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {
        self.stack.into_iter()
    }
}

impl<T: Default + Copy, const N: usize> Index<usize> for Stack<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        promise!(index < N);
        &self.stack[index]
    }
}

impl<T: Default + Copy, const N: usize> IndexMut<usize> for Stack<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        promise!(index < N);
        &mut self.stack[index]
    }
}

#[cfg(debug_assertions)]
impl<T: Default + std::fmt::Debug, const N: usize> std::fmt::Debug for Stack<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", &self.stack[..self.cursor])
    }
}
