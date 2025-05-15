use super::collector::{Collectable, Gc, GcSized};

use super::object::Objects;

pub struct Heap {
    head: Option<Objects>,
    size: usize,
    growth: usize,
    threshold: usize,
}

impl Default for Heap {
    fn default() -> Self {
        Self {
            head: None,
            size: 0,
            growth: 2,
            threshold: 1024 * 1024,
        }
    }
}

impl Heap {
    #[must_use]
    pub fn new(growth: usize, threshold: usize) -> Self {
        Self {
            head: None,
            size: 0,
            growth,
            threshold,
        }
    }

    pub fn alloc<T: GcSized, F>(&mut self, value: T, map: F) -> (Objects, Collectable<T>)
    where
        F: Fn(Collectable<T>) -> Objects,
    {
        let boxed = Box::new(Gc::new(self.head, value));
        let content = Collectable::new(boxed);
        let object = map(content);
        self.size += object.size();
        self.head = Some(object);

        (object, content)
    }

    pub fn sweep(&mut self) {
        let mut prev_obj: Option<Objects> = None;
        let mut curr_obj = self.head;

        while let Some(mut curr_ref) = curr_obj {
            let next = curr_ref.get_next();

            if curr_ref.is_marked() {
                curr_ref.unmark();
                prev_obj = curr_obj;
                curr_obj = next;
            } else {
                self.dealloc(curr_ref);
                curr_obj = next;
                if let Some(mut prev_ref) = prev_obj {
                    prev_ref.set_next(next);
                } else {
                    self.head = curr_obj;
                }
            }
        }

        if self.size <= self.threshold - self.growth {
            self.threshold = (self.size.max(1) * self.growth).max(self.threshold);
        }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub fn dealloc(&mut self, object: Objects) {
        let size = object.size();
        self.size -= size;

        match object {
            Objects::None => (),
            Objects::Array(value) => value.release(),
            Objects::String(value) => value.release(),
            Objects::Object(value) => value.release(),
            Objects::Iterator(value) => value.release(),
            Objects::Coroutine(value) => value.release(),
        }
    }

    #[must_use]
    pub fn iter(&self) -> HeapIter {
        <&Self as IntoIterator>::into_iter(self)
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        for object in &*self {
            self.dealloc(object);
        }
    }
}

impl IntoIterator for &Heap {
    type Item = Objects;

    type IntoIter = HeapIter;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter { next: self.head }
    }
}

pub struct HeapIter {
    next: Option<Objects>,
}

impl Iterator for HeapIter {
    type Item = Objects;

    fn next(&mut self) -> Option<Self::Item> {
        let n = self.next.take();

        if let Some(node) = n {
            self.next = node.get_next();
        }

        n
    }
}
