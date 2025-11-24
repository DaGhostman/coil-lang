use std::{
    alloc::{Layout, LayoutError},
    borrow::{Borrow, BorrowMut},
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use crate::garbage::{GcSized, collector::Collector};

pub struct Header(());

pub struct Reference<T>(T);

impl<T> Reference<T> {
    #[inline]
    pub fn new(value: T) -> Self {
        Self(value)
    }

    // #[inline]
    // pub fn inc(&mut self) {
    //     self.header.0 += 1;
    // }
    //
    // #[inline]
    // pub fn dec(&mut self) {
    //     if self.header.0 > 0 {
    //         self.header.0 -= 1;
    //     }
    // }

    #[inline]
    pub fn data(&self) -> &T {
        &self.0
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> AsRef<T> for Reference<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self.data()
    }
}

impl<T> AsMut<T> for Reference<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self.data_mut()
    }
}

impl<T: GcSized> GcSized for Reference<T> {
    #[inline]
    fn size(&self) -> usize {
        self.0.size()
    }
}

impl<T> From<T> for Reference<T> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

pub struct Allocated<T>(NonNull<Reference<T>>);

impl<T: GcSized> GcSized for Allocated<T> {
    #[inline]
    fn size(&self) -> usize {
        self.deref().size()
    }
}

impl<T> Deref for Allocated<T> {
    type Target = Reference<T>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl<T> DerefMut for Allocated<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

impl<T> Borrow<T> for Allocated<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self.data()
    }
}

impl<T> BorrowMut<T> for Allocated<T> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self.data_mut()
    }
}

impl<T> Allocated<T> {
    #[inline]
    pub fn new(value: *mut Reference<T>) -> Self {
        Allocated(NonNull::new(value).expect("Invalid pointer"))
    }

    #[inline]
    pub fn ptr(&self) -> usize {
        self.0.addr().into()
    }
}

// impl<T> Drop for Allocated<T> {
//     fn drop(&mut self) {
//         unsafe {
//             self.0.as_mut().dec();
//         }
//     }
// }

// impl<T> Copy for Allocated<T> {}
// impl<T> Clone for Allocated<T> {
//     fn clone(&self) -> Self {
//         *self
//     }
// }

const CHUNK_SIZE: usize = 32768; // 32k memory blocks
const BLOCK_SIZE: usize = 128; // 128 bytes per block
const BLOCKS_PER_CHUNK: usize = CHUNK_SIZE / BLOCK_SIZE;

struct Chunk {
    end: *mut u8,
    start: *mut u8,
    free_list: Vec<bool>,
}

impl Chunk {
    #[inline]
    pub fn new() -> Result<Self, LayoutError> {
        match Layout::from_size_align(CHUNK_SIZE, align_of::<u8>()) {
            Ok(layout) => {
                let ptr = unsafe { std::alloc::alloc(layout) };
                if ptr.is_null() {
                    panic!("OOM");
                }

                Ok(Self {
                    // top: ptr,
                    start: ptr,
                    end: (ptr as usize + CHUNK_SIZE) as _,
                    free_list: vec![true; BLOCKS_PER_CHUNK],
                })
            }
            Err(e) => Err(e),
        }
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(
                self.start,
                Layout::from_size_align(self.end as usize - self.start as usize, align_of::<u8>())
                    .unwrap(),
            )
        };
    }
}

#[derive(Default)]
pub struct Allocator {
    cursor: usize,
    chunks: Vec<Chunk>,
}

impl Allocator {
    #[inline]
    pub fn inner_allocate<T>(&mut self, size: usize) -> *mut Reference<T> {
        // Verify allocation fits block size
        let blocks_needed = (size + BLOCK_SIZE - 1) / BLOCK_SIZE;

        if blocks_needed > BLOCKS_PER_CHUNK {
            return unsafe {
                std::alloc::alloc_zeroed(Layout::from_size_align_unchecked(
                    size,
                    align_of::<Reference<T>>(),
                )) as *mut _
            };
        }

        // Check current chunk for free block
        if let Some(chunk) = self.chunks.get_mut(self.cursor) {
            for i in 0..(chunk.free_list.len() - blocks_needed + 1) {
                if chunk.free_list[i..i + blocks_needed].iter().all(|&f| f) {
                    let block_offset = i * BLOCK_SIZE;
                    for j in i..i + blocks_needed {
                        chunk.free_list[j] = false;
                    }

                    return unsafe {
                        chunk.start.offset(block_offset as isize) as *mut Reference<T>
                    };
                }
            }
        }

        // Create new chunk if current is full
        self.cursor = self.chunks.len();
        let new_chunk = Chunk::new().expect("Failed to allocate new chunk");
        self.chunks.push(new_chunk);

        // Allocate from first block of new chunk
        let chunk = self.chunks.last_mut().unwrap();
        for i in 0..blocks_needed {
            chunk.free_list[i] = false;
        }

        chunk.start as *mut Reference<T>
    }

    // Free a block and mark as available
    #[inline]
    pub fn free(&mut self, ptr: *mut u8, blocks: usize) {
        if let Some(chunk_index) = self.chunks.iter().position(|chunk| {
            let start = chunk.start as usize;
            let end = chunk.end as usize;
            (ptr as usize) >= start && (ptr as usize) < end
        }) {
            let chunk = &mut self.chunks[chunk_index];
            let block_index = ((ptr as usize) - chunk.start as usize) / BLOCK_SIZE;
            for n in block_index..(block_index + blocks) {
                chunk.free_list[n] = true;
            }
        }
    }

    // #[inline]
    // fn reclaim(&mut self) {
    //     self.chunks
    //         .iter_mut()
    //         .filter_map(|chunk| {
    //             for block_idx in 0..BLOCKS_PER_CHUNK {
    //                 if !chunk.free_list[block_idx] {
    //                     let block_ptr =
    //                         unsafe { chunk.start.offset(block_idx as isize * BLOCK_SIZE as isize) };
    //                     let header = unsafe { &mut *(block_ptr as *mut Header) };
    //                     return Some((block_ptr, header.1));
    //                 }
    //             }
    //
    //             None
    //         })
    //         .collect::<Vec<_>>()
    //         .iter()
    //         .for_each(|(ptr, size)| self.free(*ptr, *size));
    // }
}

// impl Drop for Allocator {
//     fn drop(&mut self) {
//         let _ = self.chunks.drain(..);
//     }
// }

pub struct Heap {
    allocator: Allocator,
}

impl Heap {
    pub fn new() -> Self {
        if let Ok(chunk) = Chunk::new() {
            let mut chunks = Vec::with_capacity(32);
            chunks.push(chunk);

            Self {
                allocator: Allocator {
                    chunks,
                    cursor: 0,
                    // growth: CHUNK_SIZE,
                },
            }
        } else {
            panic!("Unable to allocate heap")
        }
    }

    #[cfg(debug_assertions)]
    pub fn stats(&self) -> String {
        let allocator = &self.allocator;

        let mut result = format!("Chunks in use {}", allocator.chunks.len());
        result = format!(
            "{}\n Sectors {} and each {}",
            result, BLOCKS_PER_CHUNK, BLOCK_SIZE
        );
        for (idx, chunk) in allocator.chunks.iter().enumerate() {
            let used = chunk
                .free_list
                .iter()
                .filter(|v| **v == false)
                .collect::<Vec<&bool>>()
                .len();

            result = format!("{}\n\t #{} {}/{}", result, idx, used, BLOCKS_PER_CHUNK);
        }

        result
    }
}

impl Heap {
    #[inline]
    pub fn free<T: GcSized>(&mut self, value: Allocated<T>) {
        self.allocator.free(
            value.ptr() as _,
            (value.size() + BLOCK_SIZE - 1) / BLOCK_SIZE,
        );
    }

    pub fn alloc<T>(&mut self, value: T) -> Allocated<T> {
        let size = std::mem::size_of::<Reference<T>>();
        let ptr = self.allocator.inner_allocate::<T>(size);

        // Construct the Reference<T> in allocated memory
        unsafe {
            std::ptr::write(ptr, Reference::new(value));
        }

        Allocated::<T>::new(ptr)
    }

    pub fn gc(&mut self, stack: &[*mut u8]) {
        let mut collector = Collector::default();

        let boundaries = &self
            .allocator
            .chunks
            .iter()
            .map(|c| (c.start, c.end))
            .collect::<Vec<_>>();

        let mut dead_chunks = vec![];

        for (idx, chunk) in boundaries.iter().enumerate() {
            let to_free = collector.mark::<BLOCK_SIZE>(&stack, (chunk.0, chunk.1));
            if to_free.len() == BLOCKS_PER_CHUNK {
                dead_chunks.push(idx);
            } else {
                for block in to_free {
                    self.allocator.free(block, 1);
                }
            }
        }

        for ch in dead_chunks.iter().rev() {
            self.allocator.chunks.remove(*ch);
            self.allocator.cursor = self.allocator.cursor.max(1) - 1;
        }
    }
}
