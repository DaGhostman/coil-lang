use rustc_hash::{FxHashMap as HashMap, FxHasher};

use std::{
    borrow::Borrow,
    fmt::Display,
    hash::{Hash, Hasher},
};

fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

use crate::{Value, memory2::GcSized};

use super::collector::Collectable;

#[derive(Default, Debug, Clone, Copy, Hash)]
pub enum Objects {
    #[default]
    None,
    Array(Collectable<ObjArray>), // Array(HashMap<usize, usize>),
    String(Collectable<ObjString>),
    Object(Collectable<ObjInstance>),
}

impl Display for Objects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Objects::None => String::new(),
                Objects::String(value) =>
                    <Collectable<ObjString> as Borrow<ObjString>>::borrow(value).to_string(),
                Objects::Array(value) =>
                    <Collectable<ObjArray> as Borrow<ObjArray>>::borrow(value).to_string(),
                Objects::Object(value) =>
                    <Collectable<ObjInstance> as Borrow<ObjInstance>>::borrow(value).to_string(),
            }
        )
    }
}

impl Objects {
    pub fn mark(&mut self, grey: &mut Vec<Self>) {
        let marked = match self {
            Self::None => false,
            Self::Array(value) => value.mark(),
            Self::String(value) => value.mark(),
            Self::Object(value) => value.mark(),
        };

        if marked {
            grey.push(*self);
        }
    }

    pub fn unmark(&mut self) {
        match self {
            Self::None => (),
            Self::Array(value) => value.unmark(),
            Self::String(value) => value.unmark(),
            Self::Object(value) => value.unmark(),
        }
    }

    pub fn is_marked(&self) -> bool {
        match self {
            Self::None => true,
            Self::Array(value) => value.is_marked(),
            Self::String(value) => value.is_marked(),
            Self::Object(value) => value.is_marked(),
        }
    }

    pub fn mark_references(&self, grey: &mut Vec<Self>) {
        match self {
            Self::None | Self::String(_) => (),
            Self::Object(o) => o.as_ref().state.iter().for_each(|(_, v)| match v {
                Value::OBJECT(o) | Value::STRING(o) => o.mark_references(grey),
                _ => (),
            }),
            Self::Array(r) => r.as_ref().items.iter().for_each(|v| match v {
                Value::OBJECT(o) | Value::STRING(o) => o.mark_references(grey),
                _ => (),
            }),
        }
    }

    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::None => None,
            Self::Array(value) => value.get_next(),
            Self::String(value) => value.get_next(),
            Self::Object(value) => value.get_next(),
        }
    }

    pub fn set_next(&mut self, next: Option<Self>) {
        match self {
            Self::None => (),
            Self::Array(value) => value.set_next(next),
            Self::String(value) => value.set_next(next),
            Self::Object(value) => value.set_next(next),
        }
    }
}

impl GcSized for Objects {
    fn size(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Array(items) => items.size(),
            Self::String(items) => items.size(),
            Self::Object(items) => items.size(),
        }
    }
}

#[derive(Hash, Default, Debug, Clone)]
pub struct ObjArray {
    length: usize,
    items: Vec<Value>,
}

impl GcSized for ObjArray {
    fn size(&self) -> usize {
        std::mem::size_of_val(&self.length) + std::mem::size_of_val(&self.items)
    }
}

impl From<Vec<Value>> for ObjArray {
    fn from(items: Vec<Value>) -> Self {
        Self {
            length: items.len(),
            items,
        }
    }
}

impl Display for ObjArray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.items
                .iter()
                .map(Value::to_string)
                .collect::<Vec<String>>()[0..3]
                .join(", ")
        )
    }
}

#[derive(Default, Debug, Clone, Hash)]
pub struct ObjString {
    length: usize,
    hash: u64,
    contents: String,
}

impl GcSized for ObjString {
    fn size(&self) -> usize {
        std::mem::size_of_val(&self.length)
            + std::mem::size_of_val(&self.hash)
            + std::mem::size_of_val(&self.contents)
    }
}

impl From<String> for ObjString {
    fn from(value: String) -> Self {
        Self {
            length: value.len(),
            hash: calculate_hash(&value),
            contents: value,
        }
    }
}

impl Display for ObjString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.contents)
    }
}

#[derive(Default, Debug, Clone)]
pub struct ObjInstance {
    name: usize,
    hash: u64,
    state: HashMap<usize, Value>,
}

impl ObjInstance {
    pub fn new(name: usize) -> Self {
        Self {
            name,
            hash: 0,
            state: Default::default(),
        }
    }

    pub fn name(&self) -> usize {
        self.name
    }

    pub fn update(&mut self, name: usize, value: Value) {
        self.state.insert(name, value);
        self.hash = calculate_hash(self);
    }

    pub fn get(&self, name: usize) -> Option<&Value> {
        self.state.get(&name)
    }

    pub fn all(&self) -> &HashMap<usize, Value> {
        &self.state
    }
}

impl Hash for ObjInstance {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (k, v) in &self.state {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl From<&Vec<usize>> for ObjInstance {
    fn from(value: &Vec<usize>) -> Self {
        Self {
            name: 0,
            state: HashMap::from_iter(value.iter().map(|key| (*key, Default::default()))),
            hash: calculate_hash(&value),
        }
    }
}

impl GcSized for ObjInstance {
    fn size(&self) -> usize {
        // Ignore the fields as those live on separately
        std::mem::size_of_val(&self.hash)
            + std::mem::size_of_val(&self.name)
            + std::mem::size_of_val(&self.state)
    }
}

impl Display for ObjInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash)
    }
}
