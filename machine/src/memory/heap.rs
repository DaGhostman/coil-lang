use crate::{
    Object,
    garbage::{Collectable, GcSized, Rc},
};

pub struct Heap();

impl Heap {
    /// Allocate an object on the heap returning an object & a collectable
    pub fn alloc<T: GcSized, F>(value: T, map: F) -> (Object, Collectable<T>)
    where
        F: Fn(Collectable<T>) -> Object,
    {
        let boxed = Box::new(Rc::new(value));
        let content = Collectable::new(boxed);

        let object = map(content);

        #[cfg(debug_assertions)]
        eprintln!("ALLOCATED: {} bytes @ {}", object.size(), object);

        (object, content)
    }

    /// Free the provided collectable
    pub fn free<T: GcSized>(mut object: Collectable<T>) {
        if object.dec() == 0 {
            #[cfg(debug_assertions)]
            eprintln!("COLLECTING: {} bytes @ 0x{:016x}", object.size(), object.ptr().addr());
            object.release();
        }
    }
}

