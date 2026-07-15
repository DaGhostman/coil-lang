//! Reference-counted cell (unused by the live heap).

use std::cell::Cell;

use crate::garbage::GcSized;

pub struct Rc<T> {
    count: Cell<usize>,
    data: T,
}

impl<T> Rc<T> {
    pub const fn new(data: T) -> Self {
        Self {
            count: Cell::new(1),
            data,
        }
    }

    #[inline]
    pub fn inc(&self) -> usize {
        self.count.update(|ref_count| ref_count + 1);

        self.count.get()
    }

    #[inline]
    pub fn dec(&self) -> usize {
        self.count.update(|ref_count| ref_count.max(1) - 1);

        self.count.get()
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.count.get()
    }

    #[inline]
    pub fn data(&self) -> &T {
        &self.data
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T> AsRef<T> for Rc<T> {
    fn as_ref(&self) -> &T {
        self.data()
    }
}

impl<T> AsMut<T> for Rc<T> {
    fn as_mut(&mut self) -> &mut T {
        self.data_mut()
    }
}

impl<T: GcSized> GcSized for Rc<T> {
    fn size(&self) -> usize {
        use std::mem;

        mem::size_of_val(&self.count) + self.data.size()
    }
}
