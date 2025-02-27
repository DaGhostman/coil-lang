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
    constants: Interner<V>,
    // Experiment with the stack
    stack: [usize; 4096],
    sp: usize,
    // heap: Arena<V>,
}

impl<V> Debug for Memory<V>
where
    V: Eq + Hash + Clone + Default,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.stack[0..self.sp].to_vec())
    }
}

impl<'memory, V> Memory<V>
where
    V: Eq + Hash + Clone + Debug + Default,
{
    pub fn new() -> Self {
        Memory {
            constants: Interner::default(),
            // heap: Arena::with_capacity(8, Some(4)),
            sp: 0,
            stack: [0; 4096],
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
        // if self.stack.len() >= self.stack_size - 1 {
        //     return Err(MemoryError::StackOverflow);
        // }

        self.stack[self.sp] = value;
        self.sp += 1;
        // self.stack.push(value);

        Ok(())
    }

    pub fn pop(&mut self) -> usize {
        self.sp -= 1;

        self.stack[self.sp]
    }

    pub fn pop_value(&mut self) -> Option<&V> {
        let idx = self.pop();

        self.constants.lookup(idx)
    }

    pub fn peek(&self, idx: usize) -> &usize {
        &self.stack[idx]
        // self.stack.get(idx)
    }

    pub fn peek_value(&self, idx: usize) -> Option<&V> {
        let constant = self.peek(idx);

        self.constants.lookup(*constant)
    }

    // pub fn lookup(&'memory self, key: Key) -> Option<&'memory V> {
    //     self.heap.get(key)
    // }
    //
    // pub fn lookup_mut(&'memory mut self, key: Key) -> Option<&'memory mut V> {
    //     self.heap.get_mut(key)
    // }

    pub fn stack_size(&self) -> usize {
        self.sp
        // self.stack.len()
    }

    pub fn truncate(&mut self, length: usize) {
        self.sp = length;
    }

    pub fn stack_values(&self) -> Vec<V> {
        self.stack[0..self.sp]
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
        let mut mem: Memory<usize> = Memory::new();
        let k = mem.define(42);

        if let Err(e) = mem.push(k) {
            dbg!(&e);
        }
    }
}
