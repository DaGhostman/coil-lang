use std::{
    borrow::{Borrow, BorrowMut},
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use crate::garbage::{Gc, GcSized};

#[derive(Debug)]
pub struct Collectable<T>(NonNull<Gc<T>>);

impl<T> From<NonNull<Gc<T>>> for Collectable<T> {
    fn from(value: NonNull<Gc<T>>) -> Self {
        Self(value)
    }
}

impl<T> Collectable<T> {
    pub fn new(boxed: Box<Gc<T>>) -> Self {
        Self(NonNull::from(Box::leak(boxed)))
    }

    pub fn release(self) {
        let _ = unsafe { Box::from_raw(self.0.as_ptr()) };
    }

    pub fn eq(lhs: Self, rhs: Self) -> bool {
        lhs.0.eq(&rhs.0)
    }

    pub fn ptr(&self) -> NonNull<Gc<T>> {
        self.0
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
        unsafe { self.0.as_ref() }
    }
}

impl<T> DerefMut for Collectable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
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
        self.deref().data()
    }
}

impl<T> BorrowMut<T> for Collectable<T> {
    fn borrow_mut(&mut self) -> &mut T {
        unsafe { self.0.as_mut().data_mut() }
    }
}
