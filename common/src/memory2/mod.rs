use collector::{Collectable, Gc, GcSized};
use object::Objects;

pub mod collector;
pub mod object;

use std::fmt::Debug;

#[derive(Debug)]
pub enum MemoryError {
    StackOverflow,
    StackUnderflow,
}

pub struct Stack<T>
where
    T: Copy,
{
    stack: Vec<T>,
    sp: usize,
}

impl<T> Stack<T>
where
    T: Copy + Default + Debug,
{
    pub fn new(size: usize) -> Self {
        Self {
            sp: 0,
            stack: vec![Default::default(); size],
        }
    }

    pub fn pop(&mut self) -> T {
        self.sp -= 1;

        self.stack[self.sp]
    }

    pub fn npop(&mut self, n: usize) -> Vec<T> {
        let slice = &self.stack[self.sp - n..self.sp];
        self.sp -= n;

        slice.to_vec()
    }

    pub fn push(&mut self, value: T) {
        if self.stack.len() == self.sp {
            self.stack.resize(self.stack.len() + 4, Default::default());
        }

        self.stack[self.sp] = value;
        self.sp += 1;
    }

    pub fn tell(&self, offset: usize) -> usize {
        self.sp.wrapping_sub(offset)
    }

    pub fn restore(&mut self, size: usize) {
        self.stack[size] = self.stack[self.sp - 1];
        self.sp = size;
        self.sp += 1;
    }

    pub fn peek(&self, position: usize) -> T {
        self.stack[position]
    }

    pub fn copy(&mut self, src: usize, dst: usize) {
        if self.stack.len() == self.sp {
            self.stack.resize(self.stack.len() + 4, Default::default());
        }
        self.stack[dst] = self.stack[src];
    }

    pub fn copy_to_top(&mut self, src: usize) {
        self.stack[self.sp] = self.stack[src];

        self.sp += 1;
    }

    pub fn iter<'iter>(&'iter self) -> StackIterator<'iter, T> {
        StackIterator {
            cursor: 0,
            length: self.sp,
            items: self.stack[..self.sp].as_ref(),
        }
    }
}

pub struct StackIterator<'iter, T> {
    items: &'iter [T],
    length: usize,
    cursor: usize,
}

impl<'iter, T> Iterator for StackIterator<'iter, T>
where
    T: Copy,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor != self.length {
            self.cursor += 1;

            return Some(self.items[self.cursor - 1]);
        }

        None
    }
}

pub struct Heap {
    head: Option<Objects>,
    size: usize,
    growth: usize,
    threshold: usize,
}

impl Heap {
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
                    self.head = curr_obj
                }
            }
        }

        if self.size <= self.threshold - self.growth {
            self.threshold = (self.size.max(1) * self.growth).max(self.threshold);
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

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
        }
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

    type IntoIter = Iter;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter { next: self.head }
    }
}

pub struct Iter {
    next: Option<Objects>,
}

impl Iterator for Iter {
    type Item = Objects;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.next {
            self.next = node.get_next();

            return Some(node);
        }

        None
    }
}
