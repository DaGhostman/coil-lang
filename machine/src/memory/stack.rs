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
