use std::{collections::HashMap, fmt::Debug};

use crate::{Value, ValueKind};
use arena::{Arena, Key};

pub mod allocator;
pub mod arena;
pub mod ref_counter;
pub mod table;

#[derive(Clone)]
pub struct Instance<T> {
    kind: usize,
    fields: HashMap<usize, T>,
}

#[derive(Clone, Debug)]
pub struct Array<T> {
    items: HashMap<usize, T>,
}

impl<T: Copy> Array<T> {
    pub fn with_items(items: Vec<T>) -> Self {
        let mut members = HashMap::with_capacity(items.len());

        items.iter().enumerate().for_each(|(key, v)| {
            members.insert(key, *v);
        });

        Self { items: members }
    }

    pub fn item(&self, idx: usize) -> Option<&T> {
        self.items.get(&idx)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[derive(Clone)]
pub struct Str {
    len: usize,
    content: String,
}

#[derive(Clone)]
pub enum Object<T: Clone + Copy> {
    Object(Instance<T>),
    Array(Array<T>),
    String(Str),
}

pub struct Memory {
    stack: Vec<Value>,
    heap: Arena<Object<ValueKind>>,
}

impl Memory {
    pub fn new(stack_capacity: usize, heap_capacity: usize) -> Self {
        Self {
            stack: Vec::with_capacity(stack_capacity),
            heap: Arena::with_capacity(heap_capacity, None),
        }
    }

    pub fn push(&mut self, value: Value) {
        if self.stack.capacity() - 1 <= self.stack.len() {
            panic!("Stack overflow");
        }

        match value.kind() {
            ValueKind::ARRAY(key) => self.heap.reference(*key),
            _ => (),
        }

        self.stack.push(value);
    }

    pub fn pop(&mut self) -> Option<Value> {
        if self.stack.len() == 0 {
            eprintln!("Stack underflow");
        }

        let val = self.stack.pop();
        match val.map(|v| v.kind().clone()) {
            Some(ValueKind::ARRAY(key)) => self.heap.dereference(key),
            _ => (),
        }

        val
    }

    pub fn peek(&mut self, offset: usize) -> Option<&Value> {
        if self.stack.is_empty() {
            None
        } else if self.stack.len() < offset {
            None
        } else {
            self.stack.get(self.stack.len() - offset)
        }
    }

    pub fn alloc(&mut self, value: Object<ValueKind>) -> Key {
        self.heap.alloc(value)
    }

    pub fn lookup(&self, key: Key) -> Option<&Object<ValueKind>> {
        self.heap.get(key)
    }

    pub fn lookup_mut(&mut self, key: Key) -> Option<&mut Object<ValueKind>> {
        self.heap.get_mut(key)
    }

    pub fn free(&mut self, key: Key) -> Option<Object<ValueKind>> {
        if let Some(value) = self.heap.get(key).cloned() {
            self.heap.free(key);

            Some(value)
        } else {
            None
        }
    }

    pub fn truncate(&mut self, index: usize) {
        for value in self
            .stack
            .drain(index..self.stack.len())
            .collect::<Vec<Value>>()
            .iter()
            .copied()
        {
            match value.kind() {
                ValueKind::ARRAY(key) => {
                    self.heap.dereference(*key);
                }
                _ => (),
            }
        }
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    // pub fn size(&self) -> usize {
    //     let mut size = std::mem::size_of_val(&self);
    //     size += self.heap.size();
    //     size += self.stack.len() * size_of::<T>();
    //
    //     size
    // }
}

impl Debug for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}",
            self.stack
                .iter()
                .map(|v| *v.kind())
                .collect::<Vec<ValueKind>>()
        )
    }
}
