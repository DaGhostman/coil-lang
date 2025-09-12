use std::fmt::Debug;

use common::{promise, ArrayVec, ArrayVecIter};

const STACK: usize = 32;

#[repr(u8)]
#[derive(Default, Copy, Clone, PartialEq)]
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
    allocations: ArrayVec<u64, STACK>,
}

impl<T: Default> Default for Frame<T> {
    fn default() -> Self {
        Self {
            ip: 0,
            // state: FrameState::default(),
            stack: ArrayVec::default(),
            allocations: ArrayVec::default(),
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
        promise!(index < STACK);

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

    pub fn alloc(&mut self, addr: u64) {
        self.allocations.push(addr);
    }

    pub fn allocations<'iter>(&self) -> ArrayVecIter<'_, u64, STACK> {
        self.allocations.iter()
    }

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
    pub fn resume(&mut self, value: T) {
        // self.state = FrameState::PENDING;
        self.stack.push(value);
    }
    //
    // pub fn is_pending(&self) -> bool {
    //     self.state == FrameState::PENDING
    // }

    pub fn enter(&mut self, ip: usize) {
        // self.state = FrameState::default();
        self.ip = ip;
        self.stack.seek(0);
    }

    pub fn stack(&self) -> &ArrayVec<T, STACK> {
        &self.stack
    }

    pub fn stack_mut(&mut self) -> &mut ArrayVec<T, STACK> {
        &mut self.stack
    }
}

#[cfg(debug_assertions)]
impl<T: Default + Debug> Debug for Frame<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.stack)
    }
}
