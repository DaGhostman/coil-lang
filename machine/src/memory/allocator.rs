use std::{
    alloc::Layout,
    borrow::{Borrow, BorrowMut},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use common::ArrayVec;

use crate::garbage::{GcSized, Rc};

pub struct ArenaAllocated<T>(*mut Rc<T>);

impl<T> ArenaAllocated<T> {
    pub fn new(ptr: *mut Rc<T>) -> Self {
        Self(ptr)
    }

    pub fn eq(lhs: Self, rhs: Self) -> bool {
        lhs.0.eq(&rhs.0)
    }

    pub fn ptr(&self) -> *mut Rc<T> {
        self.0
    }
}

impl<T: GcSized> GcSized for ArenaAllocated<T> {
    fn size(&self) -> usize {
        self.deref().size()
    }
}

impl<T> Deref for ArenaAllocated<T> {
    type Target = Rc<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0 }
    }
}

impl<T> DerefMut for ArenaAllocated<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0 }
    }
}

impl<T> Copy for ArenaAllocated<T> {}
impl<T> Clone for ArenaAllocated<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Borrow<Rc<T>> for ArenaAllocated<T> {
    fn borrow(&self) -> &Rc<T> {
        self.deref()
    }
}

impl<T> BorrowMut<Rc<T>> for ArenaAllocated<T> {
    fn borrow_mut(&mut self) -> &mut Rc<T> {
        unsafe { self.0.as_mut().expect("Unable to obtain mutable reference") }
    }
}

#[derive(Clone)]
pub struct Chunk<T: GcSized, const B: usize> {
    data: *mut u8,
    size: usize,
    count: usize,
    layout: Layout,
    freed: ArrayVec<usize, B>,
    // prev: Option<Box<Chunk<T, B>>>,
    _phantom: PhantomData<[T; B]>,
}

// impl<T: GcSized, const B: usize> Clone for Chunk<T, B> {
//     fn clone(&self) -> Self {
//         Self {
//             head: self.head,
//             count: self.count,
//             // prev: self.prev.clone(),
//             _phantom: PhantomData::default(),
//         }
//     }
// }

#[derive(Clone)]
pub struct Allocator<T: GcSized, const B: usize> {
    size: usize,
    head: Option<Chunk<T, B>>,
}

impl<T: GcSized, const B: usize> Chunk<T, B> {
    pub fn new() -> Self {
        let size = std::mem::size_of::<T>();
        let align = std::mem::align_of::<T>();

        match std::alloc::Layout::from_size_align(size * B, align) {
            Ok(layout) => unsafe {
                let data = std::alloc::alloc(layout);

                dbg!(size);

                Chunk {
                    data,
                    size,
                    layout,
                    count: 0,
                    freed: ArrayVec::default(),
                    _phantom: PhantomData::default(),
                }
            },
            Err(e) => {
                panic!("Encountered allocation error: {}", e);
            }
        }
    }

    pub fn alloc(&mut self, value: T) -> ArenaAllocated<T> {
        let offset = if self.freed.len() == 0 {
            let aligned_offset =
                (self.count + self.size + self.layout.align() - 1) & !(self.layout.align() - 1);
            if aligned_offset + self.count > B * self.size {
                panic!("Unable to overallocate");
            }

            aligned_offset
        } else {
            *self.freed.pop()
        };

        unsafe {
            let ptr = self.data.offset(self.count as isize);
            self.count = offset;
            std::ptr::write(ptr as *mut Rc<T>, Rc::new(value));

            // panic!("OFFSET: {}", ptr.offset_from(self.data));
            // panic!("{} - {} - {:?} {} {} {} {}", self.count, self.top.addr(), self.layout, self.layout.size(), self.layout.align(), aligned_offset, self.layout.size() * B);

            ArenaAllocated(ptr as _)
        }
    }

    pub fn free(&mut self, value: ArenaAllocated<T>) {
        self.freed.push(value.ptr() as _);
    }

    // pub fn free(self) {
    //     unsafe { std::alloc::dealloc(self.data, self.layout) }
    // }

    // pub fn prev(&mut self) -> Option<Self> {
    //     self.prev.take().map(|v| *v)
    // }

    // #[inline]
    // pub fn clear(&mut self) {
    //     self.count = 0;
    // }

    // #[inline]
    // pub fn set_prev(&mut self, chunk: Self) {
    //     self.prev = Some(Box::from(chunk));
    // }

    // #[inline]
    // pub fn len(&self) -> usize {
    //     self.count
    // }
}

impl<T: GcSized, const B: usize> Drop for Chunk<T, B> {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(self.data, self.layout);
        }
    }
}

impl<T: GcSized, const B: usize> Allocator<T, B> {
    pub fn default() -> Self {
        Allocator {
            head: None,
            size: 0,
        }
    }
}

impl<T: GcSized, const B: usize> Allocator<T, B> {
    pub fn alloc(&mut self, value: T) -> ArenaAllocated<T> {
        if self.head.is_none() {
            self.head = Some(Chunk::new());
        }

        // if (self.head.len() + self.size >= B) {
        //     let mut chunk = Chunk::new();
        //     chunk.set_prev(self.head.clone());
        //     self.head = chunk;
        // }

        if let Some(chunk) = &mut self.head {
            chunk.alloc(value)
        } else {
            unreachable!("Unable to get here");
        }
    }

    pub fn free(&mut self, value: ArenaAllocated<T>) {

        if value.dec() == 0 {
            if let Some(chunk) = &mut self.head {
                #[cfg(debug_assertions)]
                eprintln!("Cleaning: {}", value.ptr().addr());
                chunk.free(value);
            } else {
                unreachable!("Attempting to free, before create");
            }
        }
    }

    // #[inline]
    // pub fn clear(&mut self) {
    //     self.head.clear();
    //     self.size = 0;
    // }
    //
    // #[inline]
    // pub fn free(self) {
    //     self.head.free();
    // }
    // pub fn clear(&mut self) {
    //     if (self.head.prev.is_some()) {
    //         let mut prev = self.head.prev();
    //         while let Some(mut p) = prev {
    //             p.free(self.allocation.2);
    //             prev = p.prev();
    //         }
    //     }
    //     self.head.clear();
    //     self.size = 0;
    // }

    // #[inline]
    // pub fn is_empty(&self) -> bool {
    //     self.size == 0
    // }
}

// impl<T: GcSized, const B: usize> Drop for Allocator<T, B> {
//     fn drop(&mut self) {
//         let mut prev = self.head.prev();
//
//         while let Some(mut p) = prev.take() {
//             prev = p.prev();
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use crate::{Allocator, ArenaAllocated, String, garbage::GcSized};

    #[test]
    fn test_usage() {
        let mut allocator: Allocator<String, 4> = Allocator::default();
        let str = "Hello, World".to_string();
        let x: ArenaAllocated<String> = allocator.alloc(str.clone().into());
        // let y: ArenaAllocated<String> = allocator.alloc("Hello, Boss!".into());

        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());
        assert_eq!(str, x.as_ref().to_string());

        assert_eq!(96, x.size());
    }
}
