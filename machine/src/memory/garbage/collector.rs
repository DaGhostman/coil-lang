//! Legacy block-marking collector sketch (unused).

use crate::{Object, ObjectType, String};

#[derive(Default)]
pub struct Collector {}

impl Collector {
    pub fn mark<const C: usize>(
        &mut self,
        stack: &[*mut u8],
        chunk_boundary: (*mut u8, *mut u8),
    ) -> Vec<*mut u8> {
        let mut to_free: Vec<*mut u8> = Vec::with_capacity(stack.len());

        let roots = stack
            .iter()
            .filter(|x| **x >= chunk_boundary.0 && **x <= chunk_boundary.1)
            .copied()
            .collect::<Vec<*mut u8>>();

        let mut block = chunk_boundary.0;
        while block < chunk_boundary.1 {
            if !roots.contains(&block) {
                to_free.push(block);
            }

            block = unsafe { block.offset(C as isize) };
        }

        to_free
    }
}
