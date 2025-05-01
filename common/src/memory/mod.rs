use collector::{Collectable, Gc, GcSized};
use object::Objects;

pub mod collector;
pub mod object;

use std::fmt::Debug;

pub struct Stack<T, const N: usize>
where
    T: Copy + PartialEq + PartialOrd,
{
    stack: [T; N],
    sp: usize,
}

impl<T, const N: usize> Default for Stack<T, N>
where
    T: Copy + Default + Debug + PartialEq + PartialOrd,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Stack<T, N>
where
    T: Copy + Default + Debug + PartialEq + PartialOrd,
{
    #[must_use]
    pub fn new() -> Self {
        Self {
            sp: 0,
            stack: [Default::default(); N],
        }
    }

    pub fn pop(&mut self) -> T {
        self.sp -= 1;
        self.stack[self.sp]
    }

    pub fn npop(&mut self, n: usize) -> &[T] {
        let boundary = self.sp;
        self.sp -= n;

        (&self.stack[self.sp..boundary]) as _
    }

    pub fn push(&mut self, value: T) {
        self.stack[self.sp] = value;
        self.sp += 1;
    }

    pub fn insert(&mut self, idx: usize, value: T) {
        self.sp = self.sp.max(idx + 1);

        self.stack[idx] = value;
    }

    pub fn tell(&self, offset: usize) -> usize {
        self.sp - offset
    }

    pub fn restore(&mut self, size: usize) {
        let val = self.stack[self.sp - 1];
        self.sp = size;
        self.stack[self.sp] = val;
        self.sp += 1;
    }

    pub fn peek(&self, offset: usize) -> T {
        self.stack[self.sp - 1 - offset]
    }

    pub fn peek_at(&self, position: usize) -> T {
        self.stack[position]
    }

    pub fn last_mut(&mut self) -> &mut T {
        &mut self.stack[self.sp - 1]
    }

    pub fn copy(&mut self, src: usize, dst: usize) {
        self.stack[dst] = self.stack[src];
    }

    pub fn copy_to_top(&mut self, src: usize) {
        self.stack[self.sp] = self.stack[src];
        self.sp += 1;
    }

    pub fn len(&self) -> usize {
        self.sp
    }

    pub fn iter(&self) -> StackIterator<T> {
        StackIterator {
            cursor: 0,
            length: self.sp,
            items: self.stack[..self.sp].to_vec(),
        }
    }

    pub fn iter_range(&self, from: usize, to: usize) -> StackIterator<T> {
        StackIterator {
            cursor: 0,
            length: self.sp,
            items: self.stack[from..to].to_vec(),
        }
    }
}

impl<T, const N: usize> IntoIterator for &Stack<T, N>
where
    T: Copy + Default + Debug + PartialEq + PartialOrd,
{
    type Item = T;

    type IntoIter = StackIterator<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug)]
pub struct StackIterator<T> {
    items: Vec<T>,
    length: usize,
    cursor: usize,
}

impl<T> Iterator for StackIterator<T>
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

    #[must_use]
    pub fn has_allocated(&self) -> bool {
        self.size > 0
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
    pub fn iter(&self) -> Iter {
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
