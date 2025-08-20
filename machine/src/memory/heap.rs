use common::{likely, unlikely};

use crate::{
    Object,
    garbage::{Collectable, Gc, GcSized},
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
        let boxed = Box::new(Gc::new(self.head, value));
        let content = Collectable::new(boxed);

        let object = map(content);

        self.size += object.size();
        self.head = Some(object);

        (object, content)
    }

    /// Free the provided object
    pub fn free(&mut self, object: Object) -> usize {
        let size = object.size();
        self.size -= size;

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

        while let Some(mut reference) = current {
            let next = reference.get_next();

            if unlikely(reference.is_marked()) {
                reference.unmark();
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
            self.threshold = (self.size.max(1) * G).max(self.threshold);
        }
    }

    pub fn usage(&self) -> f32 {
        self.size as f32 / self.threshold as f32
    }
}
