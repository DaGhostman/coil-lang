// use std::{fmt::Debug, ops::{Index, IndexMut}};
//
// use common::{ArrayVec, promise};
//
// pub struct Stack<T: Default, const N: usize> {
//     storage: ArrayVec<T, N>,
// }
//
// impl<T: Default, const N: usize> Default for Stack<T, N> {
//     fn default() -> Self {
//         Self {
//             storage: ArrayVec::default(),
//         }
//     }
// }
//
// impl<T: Default, const N: usize> Stack<T, N> {
//     pub fn push(&mut self, value: T) -> () {
//         self.storage.push(value);
//     }
//
//     pub fn pop(&mut self) -> &T {
//         promise!(0 < self.storage.len());
//
//         self.storage.pop()
//     }
//
//     pub fn peek(&self) -> &T {
//         promise!(0 < self.storage.len());
//
//         &self.storage[self.storage.len() - 1]
//     }
//
//     pub fn seek(&mut self, top: usize) -> () {
//         promise!(top <= self.storage.len());
//         self.storage.seek(top);
//     }
//
//     pub fn tell(&self) -> usize {
//         self.storage.len()
//     }
// }
//
// impl <T: Default, const N: usize> Index<usize> for Stack<T, N> {
//     type Output = T;
//
//     fn index(&self, index: usize) -> &Self::Output {
//         promise!(index < self.storage.len());
//
//         &self.storage[index]
//     }
// }
//
// impl <T: Default, const N: usize> IndexMut<usize> for Stack<T, N> {
//     fn index_mut(&mut self, index: usize) -> &mut Self::Output {
//         promise!(index < self.storage.len());
//
//         &mut self.storage[index]
//     }
// }
//
// #[cfg(debug_assertions)]
// impl <T: Default + Debug, const N: usize> Debug for Stack<T, N> {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{:?}", self.storage)
//     }
// }

use std::{
    array::IntoIter,
    ops::{Index, IndexMut},
    slice::Iter,
};

use common::promise;

pub struct Stack<T: Default, const N: usize> {
    stack: [T; N],
    cursor: usize,
}

impl<T: Default + Copy, const N: usize> Stack<T, N> {
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
        promise!(self.cursor < N);
        self.stack[self.cursor]
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        promise!(self.cursor < self.stack.len());
        promise!(self.cursor < N);
        self.stack[self.cursor] = value;
        self.cursor += 1;
    }

    #[inline]
    pub fn peek(&self) -> &T {
        promise!(self.cursor > 0);
        promise!(self.cursor < N);
        &self.stack[self.cursor - 1]
    }

    #[inline]
    pub fn seek(&mut self, idx: usize) {
        self.cursor = idx;
    }

    #[inline]
    pub fn top(&mut self) -> &mut T {
        promise!(self.cursor > 0);
        promise!(self.cursor < N);

        &mut self.stack[self.cursor - 1]
    }

    #[inline]
    pub fn tell(&self) -> usize {
        self.cursor
    }

    #[inline]
    pub fn slice(&self, start: usize, end: usize) -> &[T] {
        &self.stack[start..end.min(self.cursor)]
    }

    #[inline]
    pub fn append(&mut self, slice: &[T]) {
        self.stack[self.cursor..self.cursor + slice.len()].copy_from_slice(slice);
        self.cursor += slice.len();
    }

    pub fn as_slice(&self) -> &[T] {
        &self.stack[..self.cursor]
    }
}

// impl<T: Default + Copy, const N: usize> IntoIterator for Stack<T, N> {
//     type Item = T;
//     type IntoIter = IntoIter<T, N>;
//
//     fn into_iter(self) -> Self::IntoIter {
//         self.stack.into_iter()
//     }
// }
//
impl<T: Default + Copy, const N: usize> Index<usize> for Stack<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        &self.stack[index]
    }
}

impl<T: Default + Copy, const N: usize> IndexMut<usize> for Stack<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        promise!(index < self.stack.len());
        promise!(index < N);
        &mut self.stack[index]
    }
}

#[cfg(debug_assertions)]
impl<T: Default + std::fmt::Debug, const N: usize> std::fmt::Debug for Stack<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", &self.stack[0..self.cursor])
    }
}
