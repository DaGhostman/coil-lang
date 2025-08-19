use std::collections::HashMap;

use crate::{ArrayVec, Value2 as Value};

#[derive(Default)]
pub struct Interner {
    storage: ArrayVec<Value, 64>,
    hash: HashMap<u64, usize>,
}

impl Interner {
    pub fn intern(&mut self, value: Value) -> usize {
        
    }
}
