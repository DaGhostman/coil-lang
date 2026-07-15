//! Slice iterator with explicit cursor seek/tell.

use crate::promise;

pub struct SeekableIterator<'iter, T> {
    cursor: usize,
    items: &'iter [T],
    len: usize,
}

impl<'iter, T> SeekableIterator<'iter, T> {
    pub fn new(slice: &'iter [T]) -> Self {
        Self {
            len: slice.len(),
            items: slice,
            cursor: 0,
        }
    }

    pub fn seek(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    pub fn tell(&mut self) -> usize {
        self.cursor
    }

    pub fn add(&mut self, offset: usize) {
        self.cursor += offset;
    }

    pub fn sub(&mut self, offset: usize) {
        self.cursor -= offset;
    }
}

impl<'iter, T> Iterator for SeekableIterator<'iter, T> {
    type Item = &'iter T;
    fn next(&mut self) -> Option<Self::Item> {
        promise!(self.cursor < self.len);
        if self.cursor < self.len {
            promise!(self.cursor < self.len);
            let value = Some(&self.items[self.cursor]);
            self.cursor += 1;

            value
        } else {
            None
        }
    }
}

impl<'iter, T> From<&'iter [T]> for SeekableIterator<'iter, T> {
    fn from(value: &'iter [T]) -> Self {
        SeekableIterator::new(value)
    }
}
