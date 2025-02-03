use std::alloc::{alloc, dealloc, Layout};

pub struct Allocator {
    start: *mut u8,
    end: *mut u8,
    current: *mut u8,
}

impl Allocator {
    fn new(size: usize) -> Self {
        if let Ok(layout) = Layout::from_size_align(size, std::mem::align_of::<usize>()) {
            let start = unsafe { alloc(layout) } as *mut u8;
            let end = unsafe { start.add(size) };

            Self {
                start,
                end,
                current: start,
            }
        } else {
            panic!("Unable to recreate allocation layout.");
        }
    }

    pub fn alloc<T>(&mut self, value: T) -> *mut T {
        let align = std::mem::align_of::<T>();

        let current = (self.current as usize + align - 1) & !(align - 1) as usize;
        self.current = current as *mut u8;
        let ptr = self.current;

        self.current = unsafe { self.current.add(std::mem::size_of::<T>()) };

        ptr as *mut T
    }

    pub fn free(&mut self) {
        if let Ok(layout) = Layout::from_size_align(
            (self.end as usize) - (self.start as usize),
            std::mem::align_of::<usize>(),
        ) {
            unsafe { dealloc(self.start as *mut _, layout) };
        }
    }

    pub fn with_capacity<T>(capacity: usize) -> Self {
        Self::new(capacity * std::mem::size_of::<T>())
    }
}
