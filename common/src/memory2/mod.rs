use super::interner::Interner;
// use super::memory::arena::{Arena, Key};
use std::fmt::Debug;
use std::hash::Hash;

#[derive(Debug)]
pub enum MemoryError {
    StackOverflow,
    StackUnderflow,
}

pub struct Memory<V>
where
    V: Eq + Hash + Clone,
{
    stack_size: usize,
    constants: Interner<V>,
    stack: Vec<usize>,
    // heap: Arena<V>,
}

impl<V> Debug for Memory<V>
where
    V: Eq + Hash + Clone + Default,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.stack)
    }
}

impl<'memory, V> Memory<V>
where
    V: Eq + Hash + Clone + Debug + Default,
{
    pub fn new(stack_size: usize) -> Self {
        Memory {
            constants: Interner::default(),
            // heap: Arena::with_capacity(8, Some(4)),
            stack_size,
            stack: Vec::with_capacity(stack_size),
        }
    }

    pub fn import_constants(&mut self, constants: Vec<V>) {
        self.constants = constants.into();
    }

    pub fn define(&mut self, value: V) -> usize {
        self.constants.intern(value)
    }

    pub fn constant(&mut self, key: usize) -> Option<&V> {
        self.constants.lookup(key)
    }

    // pub fn alloc(&mut self, value: V) -> Key {
    //     self.heap.alloc(value)
    // }

    pub fn push(&mut self, value: usize) -> Result<(), MemoryError> {
        if self.stack.len() >= self.stack_size - 1 {
            return Err(MemoryError::StackOverflow);
        }

        self.stack.push(value);

        Ok(())
    }

    pub fn pop(&mut self) -> Option<usize> {
        if self.stack.is_empty() {
            return None;
        }

        self.stack.pop()
    }

    pub fn pop_value(&mut self) -> Option<&V> {
        if let Some(idx) = self.pop() {
            self.constants.lookup(idx)
        } else {
            None
        }
    }

    pub fn peek(&self, idx: usize) -> Option<&usize> {
        self.stack.get(idx)
    }

    pub fn peek_value(&self, idx: usize) -> Option<&V> {
        if let Some(constant) = self.peek(idx) {
            self.constants.lookup(*constant)
        } else {
            None
        }
    }

    // pub fn lookup(&'memory self, key: Key) -> Option<&'memory V> {
    //     self.heap.get(key)
    // }
    //
    // pub fn lookup_mut(&'memory mut self, key: Key) -> Option<&'memory mut V> {
    //     self.heap.get_mut(key)
    // }

    pub fn stack_size(&self) -> usize {
        self.stack.len()
    }

    pub fn truncate(&mut self, length: usize) {
        self.stack.truncate(length);
    }

    pub fn stack_values(&self) -> Vec<V> {
        self.stack
            .iter()
            .map(|c| self.constants.lookup(*c).cloned().unwrap_or_default())
            .collect::<Vec<V>>()
    }
}

#[cfg(test)]
mod tests {
    use super::Memory;

    #[test]
    fn initialization() {
        let mut mem: Memory<usize> = Memory::new(4);
        let k = mem.define(42);

        if let Err(e) = mem.push(k) {
            dbg!(&e);
        }
    }
}
