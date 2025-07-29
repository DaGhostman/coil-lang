use std::{fmt::Debug, ops::{AddAssign, SubAssign}};

use common::promise;

const REGISTRIES: usize = 32;


#[repr(u8)]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub enum FrameState {
    #[default]
    PENDING,
    SUSPENDED,
    STARTED,
    COMPLETE,
    TERMINATED,
}

#[derive(Copy, Clone)]
pub struct Frame<T: Default + Copy > {
    state: FrameState,
    // ---
    ip: usize,
    sp: usize,
    // ---
    registries: [T; REGISTRIES],
}

impl <T: Default + Clone + Copy> Default for Frame<T> {
    fn default() -> Self {
        Self {
            ip: 0,
            sp: 0,
            state: FrameState::default(),
            registries: [T::default(); REGISTRIES],
        }
    }
}


impl <T: Default + Copy + AddAssign + SubAssign + From<u32> > Frame<T> {
    pub fn tell(&self) -> usize {
        self.ip
    }

    pub fn returns(&self) -> usize {
        self.sp
    }

    pub fn seek(&mut self, ip: usize) -> () {
        self.ip = ip;
    }

    pub fn seek_with_stack(&mut self, ip: usize, stack: usize) -> () {
        self.ip = ip;
        self.sp = stack;
    }

    pub fn load(&self, index: usize) -> &T {
        promise!(index < REGISTRIES);

        &self.registries[index]
    }

    pub fn store(&mut self, index: usize, value: T) -> () {
        promise!(index < REGISTRIES);

        self.registries[index] = value;
    }

    pub fn inc(&mut self, index: usize) -> () {
        promise!(index < REGISTRIES);

        self.registries[index] += 1.into();
    }

    pub fn dec(&mut self, index: usize) -> () {
        promise!(index < REGISTRIES);

        self.registries[index] -= 1.into();
    }

    pub fn get(&self, index: usize) -> &T {
        promise!(index < REGISTRIES);

        &self.registries[index]
    }

    pub fn status(&self) -> FrameState {
        self.state
    }

    pub fn is(&self, state: FrameState) -> bool {
        self.state == state
    }

    pub fn suspend(&mut self) -> () {
        self.state = FrameState::SUSPENDED;
    }

    pub fn start(&mut self) -> () {
        self.state = FrameState::STARTED;
    }

    pub fn terminate(&mut self) -> () {
        self.state = FrameState::TERMINATED;
        self.recycle();
    }

    pub fn complete(&mut self) -> () {
        self.state = FrameState::COMPLETE;
        self.recycle();
    }

    pub fn resume(&mut self) -> () {
        self.state = FrameState::PENDING;
    }

    pub fn is_pending(&self) -> bool {
        self.state == FrameState::PENDING
    }

    pub fn recycle(&mut self) -> () {
        self.state = FrameState::default();
        self.ip = 0;
        self.sp = 0;
    }
}
