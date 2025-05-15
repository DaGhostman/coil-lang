use crate::{Value, interner::Interner, symbols::SymbolTable, types::Type};

#[derive(Debug, Clone)]
pub struct Data {
    constants: Interner<(Value, usize)>,
    strings: Interner<String>,
    symbols: SymbolTable,
    types: Interner<Type>,
}

impl Default for Data {
    fn default() -> Self {
        let mut types = Interner::default();
        types.intern(Type::any());
        types.intern(Type::void());
        types.intern(Type::bool());
        types.intern(Type::integer());
        types.intern(Type::float());
        types.intern(Type::string());

        let mut constants = Interner::default();
        constants.intern((Value::NONE, 0));

        Self {
            constants,
            types,
            symbols: SymbolTable::default(),
            strings: Interner::default(),
        }
    }
}

impl Data {
    pub fn add_type(&mut self, value: Type) -> usize {
        self.types.intern(value)
    }

    #[must_use] pub fn get_type(&self, index: usize) -> &Type {
        self.types.lookup(index)
    }

    #[must_use] pub fn find_type(&self, value: Type) -> usize {
        let mut this = self.types.clone();
        let idx = this.intern(value);

        debug_assert!(idx < self.types.len());

        idx
    }

    pub fn add_string(&mut self, value: String) -> usize {
        self.strings.intern(value)
    }

    #[must_use] pub fn string(&self, index: usize) -> &String {
        self.strings.lookup(index)
    }

    pub fn add_constant(&mut self, value: Value, r#type: usize) -> usize {
        self.constants.intern((value, r#type))
    }

    pub fn replace_constant(&mut self, index: usize, value: Value, r#type: usize) {
        self.constants.replace(index, (value, r#type));
    }

    #[must_use] pub fn constant(&self, index: usize) -> &Value {
        &self.constants.lookup(index).0
    }
    pub fn constant_mut(&mut self, index: usize) -> &mut Value {
        &mut self.constants.lookup_mut(index).0
    }

    #[must_use] pub fn constant_type(&self, index: usize) -> &Type {
        self.types.lookup(self.constants.lookup(index).1)
    }

    pub fn add_symbol(&mut self, symbol: String, constant: Option<usize>) -> usize {
        self.symbols.insert(symbol, constant)
    }

    #[must_use] pub fn symbol_name(&self, symbol: usize) -> &String {
        self.symbols.name(symbol)
    }

    #[must_use] pub fn symbol_constant(&self, symbol: usize) -> usize {
        self.symbols.constant(symbol)
    }

    #[must_use] pub fn symbol_constant_exists(&self, symbol: usize) -> bool {
        self.symbols.has_constant(symbol)
    }

    #[must_use] pub fn symbol_constant_type(&self, symbol: usize) -> &Type {
        let constant = self.symbol_constant(symbol);
        self.constant_type(constant)
    }

    #[must_use] pub fn symbol_exists(&self, symbol: String) -> bool {
        self.symbols.contains(symbol)
    }

    #[must_use] pub fn symbol_index(&self, symbol: String) -> usize {
        self.symbols.symbol(symbol)
    }

    #[must_use] pub fn symbol_constant_value(&self, symbol: usize) -> &Value {
        let constant = self.symbols.constant(symbol);
        &self.constants.lookup(constant).0
    }

    #[must_use] pub fn symbol_constant_value_type(&self, symbol: usize) -> &Type {
        let constant = self.symbols.constant(symbol);
        self.types.lookup(self.constants.lookup(constant).1)
    }
}

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        self.constants == other.constants
            && self.strings == other.strings
            && self.symbols == other.symbols
    }
}
