use std::{
    borrow::{Borrow, BorrowMut},
    cell::Cell,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use super::object::Objects;

pub trait GcSized {
    fn size(&self) -> usize;
}

pub struct Gc<T> {
    marked: Cell<bool>,
    next: Cell<Option<Objects>>,
    data: T,
}

impl<T> Gc<T> {
    pub const fn new(next: Option<Objects>, data: T) -> Self {
        Self {
            marked: Cell::new(false),
            next: Cell::new(next),
            data,
        }
    }

    pub fn get_next(&self) -> Option<Objects> {
        self.next.get()
    }

    pub fn set_next(&mut self, next: Option<Objects>) {
        self.next.set(next)
    }

    pub fn is_marked(&self) -> bool {
        self.marked.get()
    }

    pub fn mark(&mut self) -> bool {
        let is_not_marked = !self.marked.get();
        if is_not_marked {
            self.marked.set(true);
        }

        is_not_marked
    }

    pub fn unmark(&mut self) {
        self.marked.set(false)
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
        std::mem::size_of_val(&self.next) + std::mem::size_of_val(&self.marked) + self.data.size()
    }
}

impl<T: GcSized + Copy> GcSized for Cell<T> {
    fn size(&self) -> usize {
        self.get().size()
    }
}

#[derive(Debug, Hash)]
pub struct Collectable<T> {
    ptr: NonNull<Gc<T>>,
}

impl<T> Collectable<T> {
    pub fn new(boxed: Box<Gc<T>>) -> Self {
        Self {
            ptr: NonNull::from(Box::leak(boxed)),
        }
    }

    pub fn release(self) {
        _ = unsafe { Box::from_raw(self.ptr.as_ptr()) }
    }

    pub fn ptr_eq(lhs: Self, rhs: Self) -> bool {
        lhs.ptr.eq(&rhs.ptr)
    }
}

impl<T: GcSized> GcSized for Collectable<T> {
    fn size(&self) -> usize {
        self.deref().size()
    }
}

impl<T> Deref for Collectable<T> {
    type Target = Gc<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for Collectable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Copy for Collectable<T> {}
impl<T> Clone for Collectable<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Borrow<T> for Collectable<T> {
    fn borrow(&self) -> &T {
        &self.deref().data
    }
}

impl<T> BorrowMut<T> for Collectable<T> {
    fn borrow_mut(&mut self) -> &mut T {
        unsafe { &mut self.ptr.as_mut().data }
    }
}
