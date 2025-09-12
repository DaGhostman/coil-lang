use common::{likely, unlikely};

use crate::{
    Object,
    garbage::{Collectable, GcSized, Rc},
};

pub struct Heap<const G: usize> {
    head: Option<Object>,
    size: usize,
    threshold: usize,
}

impl<const G: usize> Default for Heap<G> {
    fn default() -> Self {
        Self {
            threshold: G,
            size: 0,
            head: None,
        }
    }
}

impl<const G: usize> Heap<G> {
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }
}

impl<const G: usize> Heap<G> {
    /// Allocate an object on the heap
    pub fn alloc<T: GcSized, F>(&mut self, value: T, map: F) -> (Object, Collectable<T>)
    where
        F: Fn(Collectable<T>) -> Object,
    {
        let boxed = Box::new(Rc::new(self.head, value));
        let content = Collectable::new(boxed);

        let object = map(content);

        self.size += object.size();
        self.head = Some(object);




        if unlikely(self.size >= self.threshold) {
            self.threshold = (self.size.max(G)).max(self.threshold * 2);
        }


        #[cfg(debug_assertions)]
        eprintln!(
            "ALLOCATED: {} bytes {} (used {}/{} bytes)",
            object.size(),
            object,
            self.size(),
            self.threshold(),
        );

        (object, content)
    }

    /// Free the provided object
    pub fn free(&mut self, object: Object) -> usize {
        let size = object.size();
        self.size -= size;

        #[cfg(debug_assertions)]
        eprintln!(
            "COLLECTING: {} bytes {} (used {}/{} bytes)",
            size,
            object,
            self.size(),
            self.threshold(),
        );

        match object {
            Object::None => (),
            Object::String(value) => value.release(),
            Object::Reference(value) => value.release(),
            Object::Coroutine(value) => value.release(),
        }

        size
    }

    /// Collect all the marked objects
    pub fn collect(&mut self) {
        let mut previous: Option<Object> = None;
        let mut current = self.head;

        while let Some(reference) = current {
            let next = reference.get_next();

            if likely(!reference.is_collectable()) {
                previous = current;
                current = next;
            } else {
                self.free(reference);

                current = next;
                if let Some(mut reference) = previous {
                    reference.set_next(next);
                } else {
                    self.head = current;
                }
            }
        }


        if unlikely(self.size <= (self.threshold - G)) {
            self.threshold = (self.threshold.max(1) / 2).max(G);
        }

    }

    pub fn usage(&self) -> f32 {
        self.size as f32 / self.threshold as f32
    }
}

impl<const G: usize> Drop for Heap<G> {
    fn drop(&mut self) {
        let mut current = self.head;
        while let Some(reference) = current {
            current = reference.get_next();
            self.free(reference);
        }
    }
}
