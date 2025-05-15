use collector::GcSized;

pub mod collector;
pub mod object;
pub mod table;

pub mod heap;
pub mod stack;

pub use heap::{Heap, HeapIter};
pub use stack::Stack;
