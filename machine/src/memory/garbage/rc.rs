use std::cell::Cell;

use crate::{garbage::GcSized, Object};

pub struct Rc<T> {
    count: Cell<usize>,
    next: Cell<Option<Object>>,
    data: T,
}

impl <T> Rc<T> {
    pub const fn new(next: Option<Object>, data: T) -> Self {
        Self {
            count: Cell::new(1),
            next: Cell::new(next),
            data,
        }
    }

    pub fn inc(&mut self) -> usize {
        self.count.update(|ref_count| ref_count + 1);
        
        self.count.get()
    }

    pub fn dec(&mut self) -> usize {
        if self.count.get() > 0 {
            self.count.update(|ref_count| ref_count - 1);
        }

        self.count.get()
    }

    pub fn is_collectable(&self) -> bool {
        self.count.get() == 0
    }

    pub fn would_be_collectable(&self) -> bool {
        (self.count.get() - 1) == 0
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

impl <T> AsRef<T> for Rc<T> {
    fn as_ref(&self) -> &T {
        self.data()
    }
}

impl <T> AsMut<T> for Rc<T> {
    fn as_mut(&mut self) -> &mut T {
        self.data_mut()
    }
}

impl <T: GcSized> GcSized for Rc<T> {
    fn size(&self) -> usize {
        use std::mem;

        mem::size_of_val(&self.next) + mem::size_of_val(&self.count) + self.data.size()
    }
}
