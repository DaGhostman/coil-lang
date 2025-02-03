use std::{
    fmt::Display,
    hash::{DefaultHasher, Hash, Hasher},
};

use memory::arena::Key;

pub mod error;
pub mod interner;
pub mod memory;
pub mod opcodes;

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    #[default]
    NONE,
    BOOLEAN(bool),
    INTEGER(i64),
    FLOAT(f64),
    STRING(usize),
    ARRAY(Key),
    RANGE(i64, i64),
    FILE(usize),
}

impl Eq for ValueKind {}

impl PartialOrd for ValueKind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (ValueKind::INTEGER(lhs), ValueKind::INTEGER(rhs)) => lhs.partial_cmp(rhs),
            (ValueKind::FLOAT(lhs), ValueKind::FLOAT(rhs)) => lhs.partial_cmp(rhs),
            (ValueKind::BOOLEAN(lhs), ValueKind::BOOLEAN(rhs)) => lhs.partial_cmp(rhs),
            (ValueKind::NONE, ValueKind::NONE) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }
}

impl Hash for ValueKind {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            ValueKind::NONE => {
                "void".hash(state);
            }
            ValueKind::BOOLEAN(value) => {
                "b".hash(state);
                value.hash(state);
            }
            ValueKind::INTEGER(value) => {
                "i".hash(state);
                value.hash(state);
            }
            ValueKind::FLOAT(value) => {
                "f".hash(state);
                (value.trunc() as isize).hash(state);
                (value.fract() as isize).hash(state);
            }
            ValueKind::STRING(value) => {
                "s".hash(state);
                value.hash(state);
            }
            ValueKind::ARRAY(value) => {
                "a".hash(state);
                value.hash(state);
            }
            ValueKind::RANGE(start, end) => {
                "r".hash(state);
                start.hash(state);
                end.hash(state);
            }
            ValueKind::FILE(fd) => {
                "fs".hash(state);
                fd.hash(state);
            }
        }
    }
}

impl Display for ValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ValueKind::INTEGER(int) => format!("int({})", int),
                ValueKind::FLOAT(f) => format!("float({:.?})", f),
                ValueKind::NONE => String::from("void"),
                ValueKind::BOOLEAN(b) => format!("bool({})", b),
                ValueKind::STRING(s) => format!("string({})", s),
                ValueKind::ARRAY(a) => format!("arr({})", a),
                ValueKind::RANGE(start, end) => format!("range({}, {})", start, end),
                ValueKind::FILE(fd) => format!("file({})", fd),
            }
        )
    }
}

#[derive(Clone, Copy, Default)]
pub struct Value {
    kind: ValueKind,
    hash: u64,
}

impl Value {
    pub fn new(kind: ValueKind) -> Self {
        let mut hasher = DefaultHasher::default();
        kind.hash(&mut hasher);
        Self {
            kind,
            hash: hasher.finish(),
        }
    }

    pub fn kind(&self) -> &ValueKind {
        &self.kind
    }

    pub fn hash(&self) -> &u64 {
        &self.hash
    }
}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for Value {}
