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
        // Iterator contract: return `None` when exhausted (do not assert).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_all_elements_in_order() {
        let items = [10, 20, 30];
        let mut it = SeekableIterator::new(&items);
        assert_eq!(it.next(), Some(&10));
        assert_eq!(it.next(), Some(&20));
        assert_eq!(it.next(), Some(&30));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn seek_and_tell_round_trip() {
        let items = [1, 2, 3, 4];
        let mut it = SeekableIterator::from(items.as_slice());
        assert_eq!(it.tell(), 0);
        it.seek(2);
        assert_eq!(it.tell(), 2);
        assert_eq!(it.next(), Some(&3));
        assert_eq!(it.tell(), 3);
    }

    #[test]
    fn add_and_sub_adjust_cursor() {
        let items = [5, 6, 7, 8];
        let mut it = SeekableIterator::new(&items);
        it.add(2);
        assert_eq!(it.next(), Some(&7));
        it.sub(1);
        assert_eq!(it.tell(), 2);
        assert_eq!(it.next(), Some(&7));
    }

    #[test]
    fn empty_slice_yields_none() {
        let items: [i32; 0] = [];
        let mut it = SeekableIterator::new(&items);
        assert_eq!(it.next(), None);
    }
}
