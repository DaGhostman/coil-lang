use ahash::HashMap;

use crate::interner::Interner;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SymbolTable {
    names: Interner<String>,
    mapping: HashMap<usize, usize>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            names: Interner::default(),
            mapping: HashMap::default(),
        }
    }

    pub fn insert(&mut self, symbol: String, constant: Option<usize>) -> usize {
        let idx = self.names.intern(symbol);
        if let Some(constant) = constant {
            self.mapping.entry(idx).or_insert(constant);
        }

        idx
    }

    pub fn constant(&self, symbol: usize) -> Option<&usize> {
        self.mapping.get(&symbol)
    }

    pub fn name(&self, symbol: usize) -> Option<&String> {
        self.names.lookup(symbol)
    }

    pub fn dump(&self) -> Vec<(usize, Option<&usize>, String)> {
        self.names
            .dump()
            .iter()
            .enumerate()
            .map(|(i, v)| (i, self.mapping.get(&i), v.clone()))
            .collect::<Vec<(usize, Option<&usize>, String)>>()
    }
}
