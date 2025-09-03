use std::hash::{Hash, Hasher};

use common::Value;
use rustc_hash::FxHasher;

use crate::{
    Coroutine, Reference, String,
    garbage::{Collectable, GcSized},
};

pub fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

#[derive(Debug, Copy, Clone)]
pub enum ObjectType {
    None,
    String,
    Coroutine,
    Reference,

    __INVALID,
}

impl ObjectType {
    fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::String => 1,
            Self::Coroutine => 2,
            Self::Reference => 3,
            Self::__INVALID => 255,
        }
    }

    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => ObjectType::None,
            1 => ObjectType::String,
            2 => ObjectType::Coroutine,
            3 => ObjectType::Reference,
            _ => ObjectType::__INVALID,
        }
    }
}

#[derive(Default, Copy, Clone)]
pub enum Object {
    #[default]
    None,
    String(Collectable<String>),
    Reference(Collectable<Reference>),
    Coroutine(Collectable<Coroutine<Value>>),
}

impl GcSized for Object {
    fn size(&self) -> usize {
        match self {
            Self::None => 0,
            Self::String(value) => value.size(),
            Self::Reference(value) => value.size(),
            Self::Coroutine(value) => value.size(),
        }
    }
}

impl Object {
    pub fn mark(&mut self) {
        match self {
            Self::None => false,
            Self::String(value) => value.mark(),
            Self::Reference(value) => value.mark(),
            Self::Coroutine(value) => value.mark(),
        };
    }

    pub fn unmark(&mut self) {
        match self {
            Self::None => (),
            Self::String(value) => value.unmark(),
            Self::Reference(value) => value.unmark(),
            Self::Coroutine(value) => value.unmark(),
        }
    }

    pub fn is_marked(&self) -> bool {
        match self {
            Self::None => true,
            Self::String(value) => value.is_marked(),
            Self::Reference(value) => value.is_marked(),
            Self::Coroutine(value) => value.is_marked(),
        }
    }

    pub fn mark_reference(&mut self) {
        match self {
            Self::None | Self::Reference(..) | Self::String(..) | Self::Coroutine(..) => (),
        }
    }

    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::None => None,
            Self::String(value) => value.get_next(),
            Self::Reference(value) => value.get_next(),
            Self::Coroutine(value) => value.get_next(),
        }
    }

    pub fn set_next(&mut self, next: Option<Object>) {
        match self {
            Self::String(value) => value.set_next(next),
            Self::Reference(value) => value.set_next(next),
            Self::Coroutine(value) => value.set_next(next),
            _ => (),
        }
    }
}

impl std::fmt::Display for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "0x{:016x}",
            match self {
                Object::None => 0,
                Object::String(value) => value.ptr().as_ptr() as usize,
                Object::Coroutine(value) => value.ptr().as_ptr() as usize,
                Object::Reference(value) => value.ptr().as_ptr() as usize,
            }
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "Object::None"),
            Self::String(value) => write!(f, "{:?}", value),
            Self::Reference(value) => write!(f, "{:?}", value),
            Self::Coroutine(value) => write!(f, "#{:?}", value.ptr().as_ptr() as usize),
        }
    }
}
