// pub mod collectable;
// pub mod gc;
// mod rc;

use std::cell::Cell;

// pub use collectable::*;

pub trait GcSized {
    fn size(&self) -> usize;
}

impl<T: GcSized + Copy> GcSized for Cell<T> {
    fn size(&self) -> usize {
        self.get().size()
    }
}
