use rustc_hash::FxHashMap as HashMap;

use crate::interner::Interner;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SymbolTable {
    names: Interner<String>,
    mapping: HashMap<usize, usize>,
}

impl SymbolTable {
    #[must_use]
    pub fn new() -> Self {
        Self {
            names: Interner::default(),
            mapping: HashMap::default(),
        }
    }

    pub fn insert(&mut self, symbol: String, constant: Option<usize>) -> usize {
        let idx = self.names.intern(symbol);
        if let Some(constant) = constant {
            self.mapping
                .entry(idx)
                .and_modify(|val| {
                    *val = constant;
                })
                .or_insert(constant);
        }

        idx
    }

    #[must_use] pub fn constant(&self, symbol: usize) -> usize {
        self.mapping[&symbol]
    }

    #[must_use] pub fn has_constant(&self, symbol: usize) -> bool {
        self.mapping.contains_key(&symbol)
    }

    #[must_use] pub fn name(&self, symbol: usize) -> &String {
        self.names.lookup(symbol)
    }

    #[must_use] pub fn symbol(&self, symbol: String) -> usize {
        let mut interner = self.names.clone();
        interner.intern(symbol)
    }

    #[must_use] pub fn contains(&self, symbol: String) -> bool {
        let mut interner = self.names.clone();
        self.mapping.contains_key(&interner.intern(symbol))
    }
}
