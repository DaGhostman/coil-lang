//! Intrusive-list GC cell (unused; live heap uses `heap::Gc`).

use std::cell::Cell;

use common::unlikely;

use crate::{Object, garbage::GcSized};

pub struct Gc<T> {
    marked: Cell<bool>,
    next: Cell<Option<Object>>,
    data: T,
}

impl<T> Gc<T> {
    pub const fn new(next: Option<Object>, data: T) -> Self {
        Self {
            marked: Cell::new(false),
            next: Cell::new(next),
            data,
        }
    }

    pub fn is_marked(&self) -> bool {
        self.marked.get()
    }

    pub fn mark(&mut self) -> bool {
        if unlikely(!self.marked.get()) {
            self.marked.set(true);
        }

        true
    }

    pub fn unmark(&mut self) {
        self.marked.set(false);
    }

    pub fn set_next(&mut self, next: Option<Object>) {
        self.next.set(next);
    }

    pub fn get_next(&self) -> Option<Object> {
        self.next.get()
    }

    pub fn data(&self) -> &T {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T> AsRef<T> for Gc<T> {
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<T> AsMut<T> for Gc<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: GcSized> GcSized for Gc<T> {
    fn size(&self) -> usize {
        use std::mem;

        mem::size_of_val(&self.next) + mem::size_of_val(&self.marked) + self.data.size()
    }
}
