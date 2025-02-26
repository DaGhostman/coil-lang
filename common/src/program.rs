use crate::{Value, interner::Interner, symbols::SymbolTable};
use core::fmt::Debug;

#[derive(Clone, Default, PartialEq)]
pub struct Program<T> {
    code: Vec<T>,
    constants: Interner<Value>,
    strings: Interner<String>,
    symbols: SymbolTable,
}

impl<T> Debug for Program<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.code.iter().for_each(|code| {
            let _ = write!(f, "{:?} ", code);
        });

        write!(f, "")
    }
}

impl<T> Program<T>
where
    T: Clone,
{
    pub fn new(code: Vec<T>) -> Self {
        Self {
            code,
            constants: Interner::default(),
            strings: Interner::default(),
            symbols: SymbolTable::default(),
        }
    }

    pub fn add_string(&mut self, value: String) -> usize {
        self.strings.intern(value)
    }

    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.intern(value)
    }

    pub fn with_code(&mut self, code: Vec<T>) -> bool {
        let len = self.code.len();
        self.code = code;

        len == self.code.len()
    }

    pub fn with_constants(&mut self, constants: Vec<Value>) {
        self.constants = constants.into();
    }

    pub fn with_symbols(&mut self, symbols: SymbolTable) {
        self.symbols = symbols;
    }

    pub fn push(&mut self, instruction: T) {
        self.code.push(instruction);
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.code.get(index)
    }

    pub fn constant(&self, idx: usize) -> Option<&Value> {
        self.constants.lookup(idx)
    }

    pub fn string(&self, idx: usize) -> Option<&String> {
        self.strings.lookup(idx)
    }

    pub fn symbol_name(&self, idx: usize) -> Option<&String> {
        self.symbols.name(idx)
    }

    pub fn symbol_constant(&self, idx: usize) -> Option<&usize> {
        self.symbols.constant(idx)
    }

    pub fn symbols(&self) -> SymbolTable {
        self.symbols.clone()
    }

    pub fn symbol(&mut self, symbol: String, constant: Option<usize>) -> usize {
        self.symbols.insert(symbol, constant)
    }

    pub fn len(&self) -> usize {
        self.code.len()
    }

    pub fn code(&self) -> &[T] {
        self.code.as_slice()
    }

    pub fn get_constants(&self) -> Vec<Value> {
        self.constants.dump()
    }
}
