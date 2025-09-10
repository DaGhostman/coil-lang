use std::slice;

pub struct SeekableIterator<'iter, T> {
    cursor: usize,
    items: &'iter [T],
    len: usize,
}

impl <'iter, T> SeekableIterator<'iter, T> {
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

impl <'iter, T>Iterator for SeekableIterator<'iter, T> {
    type Item = &'iter T;
    fn next(&mut self) -> Option<Self::Item> {
        let mut value = None;
        if self.cursor < self.len {
            value = Some(&self.items[self.cursor]);
            self.cursor += 1;
        }

        value
    }
}

impl <'iter, T> From<&'iter [T]> for SeekableIterator<'iter, T> {
    fn from(value: &'iter [T]) -> Self {
        SeekableIterator::new(value)
    }
}
