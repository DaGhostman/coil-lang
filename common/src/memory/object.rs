use rustc_hash::{FxHashMap as HashMap, FxHasher};

use std::{
    borrow::Borrow,
    fmt::Display,
    hash::{Hash, Hasher},
    vec::IntoIter,
};

fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

use crate::{Value, memory::GcSized};

use super::collector::Collectable;

#[derive(Default, Debug, Clone, Copy, Hash)]
pub enum Objects {
    #[default]
    None,
    Array(Collectable<ObjArray>), // Array(HashMap<usize, usize>),
    Iterator(Collectable<ObjIterator>),
    String(Collectable<ObjString>),
    Object(Collectable<ObjInstance>),
    Coroutine(Collectable<ObjCoroutine>),
}

impl Display for Objects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Objects::None => String::new(),
                Objects::Iterator(value) =>
                    <Collectable<ObjIterator> as Borrow<ObjIterator>>::borrow(value).to_string(),
                Objects::String(value) =>
                    <Collectable<ObjString> as Borrow<ObjString>>::borrow(value).to_string(),
                Objects::Array(value) =>
                    <Collectable<ObjArray> as Borrow<ObjArray>>::borrow(value).to_string(),
                Objects::Object(value) =>
                    <Collectable<ObjInstance> as Borrow<ObjInstance>>::borrow(value).to_string(),
                Objects::Coroutine(value) =>
                    <Collectable<ObjCoroutine> as Borrow<ObjCoroutine>>::borrow(value).to_string(),
            }
        )
    }
}

impl Objects {
    pub fn mark(&mut self, grey: &mut Vec<Self>) {
        let marked = match self {
            Self::None => false,
            Self::Array(value) => {
                for item in &mut value.as_mut().items {
                    match item {
                        Value::OBJECT(Objects::Object(val)) => {
                            val.mark();
                        }
                        Value::OBJECT(Objects::Array(val)) => {
                            val.mark();
                        }
                        Value::OBJECT(Objects::Iterator(val)) => {
                            val.mark();
                        }
                        _ => (),
                    }
                }
                value.mark()
            }
            Self::String(value) => value.mark(),
            Self::Object(value) => {
                for value in value.as_mut().state.values_mut() {
                    match *value {
                        Value::OBJECT(Objects::Object(mut val)) => {
                            val.mark();
                        }
                        Value::OBJECT(Objects::Array(mut val)) => {
                            val.mark();
                        }
                        Value::OBJECT(Objects::Iterator(mut val)) => {
                            val.mark();
                        }
                        _ => (),
                    }
                }
                value.mark()
            }
            Self::Iterator(value) => {
                match value.as_mut().iterable {
                    Value::OBJECT(Objects::Object(mut val)) => {
                        val.mark();
                    }
                    Value::OBJECT(Objects::Array(mut val)) => {
                        val.mark();
                    }
                    Value::OBJECT(Objects::Iterator(mut val)) => {
                        val.mark();
                    }
                    _ => (),
                }
                value.mark()
            }
            Self::Coroutine(i) => {
                match i.as_mut().value {
                    Value::OBJECT(mut v) | Value::STRING(mut v) => {
                        v.mark(grey);
                    }
                    _ => (),
                }

                for val in &mut i.as_mut().stack {
                    match val {
                        Value::OBJECT(v) | Value::STRING(v) => {
                            v.mark(grey);
                        }
                        _ => (),
                    }
                }

                i.mark()
            }
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
            Self::Iterator(value) => value.unmark(),
            Self::Coroutine(value) => value.unmark(),
        }
    }

    #[must_use]
    pub fn is_marked(&self) -> bool {
        match self {
            Self::None => true,
            Self::Array(value) => value.is_marked(),
            Self::String(value) => value.is_marked(),
            Self::Object(value) => value.is_marked(),
            Self::Iterator(value) => value.is_marked(),
            Self::Coroutine(value) => value.is_marked(),
        }
    }

    pub fn mark_references(&self, grey: &mut Vec<Self>) {
        match self {
            Self::None | Self::String(_) => (),
            Self::Object(o) => o.as_ref().state.iter().for_each(|(_, v)| match v {
                Value::OBJECT(o) | Value::STRING(o) => o.mark_references(grey),
                _ => (),
            }),
            Self::Array(r) => {
                for item in r.as_ref().items.as_slice() {
                    match item {
                        Value::OBJECT(val) | Value::STRING(val) => val.mark_references(grey),
                        _ => (),
                    }
                }
                // r.as_ref().items.iter().for_each(|v| match v {
                // Value::OBJECT(o) | Value::STRING(o) => o.mark_references(grey),
                //     _ => (),
                // })
            }
            Self::Iterator(i) => match i.as_ref().iterable {
                Value::OBJECT(v) | Value::STRING(v) => v.mark_references(grey),
                _ => (),
            },
            Self::Coroutine(i) => {
                match i.as_ref().value {
                    Value::OBJECT(v) | Value::STRING(v) => v.mark_references(grey),
                    _ => (),
                }

                for val in &i.as_ref().stack {
                    match val {
                        Value::OBJECT(v) | Value::STRING(v) => v.mark_references(grey),
                        _ => (),
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::None => None,
            Self::Array(value) => value.get_next(),
            Self::String(value) => value.get_next(),
            Self::Object(value) => value.get_next(),
            Self::Iterator(value) => value.get_next(),
            Self::Coroutine(value) => value.get_next(),
        }
    }

    pub fn set_next(&mut self, next: Option<Self>) {
        match self {
            Self::None => (),
            Self::Array(value) => value.set_next(next),
            Self::String(value) => value.set_next(next),
            Self::Object(value) => value.set_next(next),
            Self::Iterator(value) => value.set_next(next),
            Self::Coroutine(value) => value.set_next(next),
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
            Self::Iterator(items) => items.size(),
            Self::Coroutine(items) => items.size(),
        }
    }
}

#[derive(Hash, Default, Debug, Clone)]
pub struct ObjArray {
    length: usize,
    items: Vec<Value>,
}

impl ObjArray {
    #[must_use]
    pub fn len(&self) -> usize {
        self.length
    }

    #[must_use]
    pub fn item(&self, index: usize) -> Value {
        self.items[index]
    }
}

impl IntoIterator for ObjArray {
    type Item = Value;

    type IntoIter = IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
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

impl ObjString {
    #[must_use]
    pub fn len(&self) -> usize {
        self.length
    }

    #[must_use]
    pub fn hash(&self) -> u64 {
        self.hash
    }
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
pub struct ObjIterator {
    hash: u64,
    iterable: Value,
    cursor: usize,
}

impl Hash for ObjIterator {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "iter".hash(state);
        self.iterable.hash(state);
    }
}

impl ObjIterator {
    #[must_use]
    pub fn new(iterable: Value) -> Self {
        ObjIterator {
            hash: 0,
            iterable,
            cursor: 0,
        }
    }

    #[must_use]
    pub fn tell(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        let length = match self.iterable {
            Value::OBJECT(Objects::Array(item)) => item.as_ref().len(),
            Value::OBJECT(Objects::String(item)) => item.as_ref().len(),
            Value::OBJECT(Objects::Object(_)) => {
                eprintln!("Objects are not iterable.. yet");
                0
            }
            _ => {
                panic!("Non iterable type {}", self.iterable)
            }
        };

        length > self.cursor
    }

    pub fn next(&mut self) {
        self.cursor += 1;
    }

    #[must_use]
    pub fn get(&self) -> Value {
        match self.iterable {
            Value::OBJECT(Objects::Array(arr)) => arr.as_ref().item(self.cursor),
            _ => Value::default(),
        }
    }
}

impl GcSized for ObjIterator {
    fn size(&self) -> usize {
        std::mem::size_of_val(&self.hash)
            + std::mem::size_of_val(&self.cursor)
            + std::mem::size_of_val(&self.iterable)
    }
}

impl Display for ObjIterator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "iter({})", self.iterable)
    }
}

#[derive(Default, Debug, Clone)]
pub struct ObjInstance {
    name: usize,
    hash: u64,
    state: HashMap<usize, Value>,
}

impl ObjInstance {
    #[must_use]
    pub fn new(name: usize) -> Self {
        Self {
            name,
            hash: 0,
            state: Default::default(),
        }
    }

    #[must_use]
    pub fn name(&self) -> usize {
        self.name
    }

    pub fn update(&mut self, name: usize, value: Value) {
        self.state.insert(name, value);
        self.hash = calculate_hash(self);
    }

    #[must_use]
    pub fn get(&self, name: usize) -> Option<&Value> {
        self.state.get(&name)
    }

    #[must_use]
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

#[derive(Default, Debug)]
pub struct ObjCoroutine {
    ip: usize,
    stack: Vec<Value>,

    value: Value,
}

impl ObjCoroutine {
    #[must_use] pub fn get(&self) -> Value {
        self.value
    }
    pub fn set(&mut self, value: Value) {
        self.value = value;
    }

    #[must_use] pub fn resume(&self) -> (usize, &Vec<Value>) {
        (self.ip, &self.stack)
    }

    pub fn suspend(&mut self, ip: usize, stack: Vec<Value>) {
        self.ip = ip;
        self.stack = stack;
    }
}

impl GcSized for ObjCoroutine {
    fn size(&self) -> usize {
        std::mem::size_of_val(&self.ip)
            + std::mem::size_of_val(&self.stack)
            + std::mem::size_of_val(&self.value)
    }
}

impl Hash for ObjCoroutine {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ip.hash(state);
        self.stack.hash(state);
        self.value.hash(state);
    }
}

impl Display for ObjCoroutine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
