use std::{fmt::Debug, mem::MaybeUninit};

use common::{promise, unlikely, ArrayVec};

use crate::{Allocator, ArenaAllocated, Object};

const STACK: usize = 16;
const HEAP: usize= 1024;

#[repr(u8)]
#[derive(Default, Copy, Clone)]
pub enum FrameState {
    #[default]
    PENDING,
    SUSPENDED,
    STARTED,
    COMPLETE,
    TERMINATED,
}

#[derive(Clone)]
pub struct Frame<T: Default> {
    // state: FrameState,
    // ---
    ip: usize,
    // ---
    stack: ArrayVec<T, STACK>,
    allocator: Allocator<Object, HEAP>,
    // allocations: ArrayVec<u64, STACK>,
}

impl<T: Default> Default for Frame<T> {
    fn default() -> Self {
        Self {
            ip: 0,
            // state: FrameState::default(),
            stack: ArrayVec::default(),
            allocator: Allocator::default(),
            // allocations: ArrayVec::default(),
        }
    }
}

impl<T: Default> Frame<T> {
    pub fn tell(&self) -> usize {
        self.ip
    }

    pub fn seek(&mut self, ip: usize) {
        self.ip = ip;
    }

    pub fn load(&self, index: usize) -> &T {
        &self.stack[index]
    }

    pub fn store(&mut self, index: usize, value: T) {
        self.stack[index] = value;
    }

    pub fn get(&self, index: usize) -> &T {
        &self.stack[index]
    }

    pub fn push(&mut self, value: T) {
        self.stack.push(value);
    }

    pub fn pop(&mut self) -> &T {
        self.stack.pop()
    }

    pub fn peek(&self) -> &T {
        self.stack.get()
    }

    pub fn top(&mut self) -> &mut T {
        self.stack.get_mut()
    }

    pub fn alloc(&mut self, value: Object) -> ArenaAllocated<Object> {
        self.allocator.alloc(value)
    }
    pub fn free(&mut self, value: ArenaAllocated<Object>) {
        self.allocator.free(value)
    }

    // #[inline]
    // pub fn len(&self) -> usize {
    //     self.stack.len()
    // }

    // pub fn alloc(&mut self, addr: u64) {
    //     self.allocations.push(addr);
    // }
    //
    // pub fn allocations<'iter>(&self) -> ArrayVecIter<'_, u64, STACK> {
    //     self.allocations.iter()
    // }

    // pub fn status(&self) -> FrameState {
    //     self.state
    // }
    //
    // pub fn is(&self, state: FrameState) -> bool {
    //     self.state == state
    // }
    //
    // pub fn suspend(&mut self) {
    //     self.state = FrameState::SUSPENDED;
    // }
    //
    // pub fn start(&mut self) {
    //     self.state = FrameState::STARTED;
    // }
    //
    // pub fn terminate(&mut self) {
    //     self.state = FrameState::TERMINATED;
    // }
    //
    // pub fn complete(&mut self) {
    //     self.state = FrameState::COMPLETE;
    // }
    //
    #[inline]
    pub fn resume(&mut self, value: T) {
        // self.state = FrameState::PENDING;
        self.stack.push(value);
    }
    //
    // pub fn is_pending(&self) -> bool {
    //     self.state == FrameState::PENDING
    // }

    #[inline]
    pub fn enter(&mut self) {
        // self.state = FrameState::default();
        self.seek(0);
        self.stack.clear();
        // if !self.allocator.is_empty() {
        //     self.allocator.clear();
        // }
    }

    pub fn stack(&self) -> &ArrayVec<T, STACK> {
        &self.stack
    }

    pub fn stack_mut(&mut self) -> &mut ArrayVec<T, STACK> {
        &mut self.stack
    }
}

// impl <T: Default> Drop for Frame<T> {
//     fn drop(&mut self) {
//         self.allocator.clear();
//     }
// }

#[cfg(debug_assertions)]
impl<T: Default + Debug> Debug for Frame<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.stack)
    }
}
