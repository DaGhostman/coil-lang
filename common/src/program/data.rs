use std::ffi::c_void;

use crate::{Value, interner::Interner, symbols::SymbolTable};

#[derive(Debug, Default, Clone)]
pub struct Data {
    constants: Interner<Value>,
    strings: Interner<String>,
    symbols: SymbolTable,

    pointers: Vec<*mut c_void>,
}

impl Data {
    pub fn add_string(&mut self, value: String) -> usize {
        self.strings.intern(value)
    }

    pub fn string(&self, index: usize) -> &String {
        self.strings.lookup(index)
    }

    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.intern(value)
    }

    pub fn constant(&self, index: usize) -> &Value {
        self.constants.lookup(index)
    }
    pub fn constant_mut(&mut self, index: usize) -> &mut Value {
        self.constants.lookup_mut(index)
    }

    pub fn add_symbol(&mut self, symbol: String, constant: Option<usize>) -> usize {
        self.symbols.insert(symbol, constant)
    }

    pub fn symbol_name(&self, symbol: usize) -> &String {
        self.symbols.name(symbol)
    }

    pub fn symbol_constant(&self, symbol: usize) -> usize {
        self.symbols.constant(symbol)
    }

    pub fn symbol_constant_value(&self, symbol: usize) -> &Value {
        let constant = self.symbols.constant(symbol);
        self.constants.lookup(constant)
    }

    pub fn add_pointer(&mut self, ptr: *mut c_void) -> usize {
        self.pointers.push(ptr);

        self.pointers.len() - 1
    }

    pub fn pointer(&self, index: usize) -> Option<*mut c_void> {
        self.pointers.get(index).copied()
    }

    pub fn dump(&self) {
        dbg!(self);
    }
}

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        self.constants == other.constants
            && self.strings == other.strings
            && self.symbols == other.symbols
    }
}
