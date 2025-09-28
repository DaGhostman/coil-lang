use common::promise;

use crate::{garbage::GcSized};

const STACK_SIZE: usize = 128;

pub struct Coroutine<T: Default>((usize, usize), [T; STACK_SIZE], usize);

impl<T: Default + Copy> Coroutine<T> {
    pub fn new(frame: (usize, usize), stack: &[T]) -> Self {
        promise!(stack.len() <= STACK_SIZE);

        let mut storage = [T::default(); STACK_SIZE];
        storage[..stack.len()].copy_from_slice(stack);

        Self(frame, storage, stack.len())
    }

    #[inline]
    pub fn ip(&self) -> usize{
        self.0.0
    }

    #[inline]
    pub fn sp(&self) -> usize {
        self.0.1
    }

    #[inline]
    pub fn stack(&self) -> &[T] {
        &self.1[..self.2]
    }
}

impl<T: Default> GcSized for Coroutine<T> {
    #[inline]
    fn size(&self) -> usize {
        use std::mem::size_of_val;

        size_of_val(&self.0) //+ (size_of::<T>() * self.0.tell())
    }
}
