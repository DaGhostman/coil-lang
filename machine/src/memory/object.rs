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
}

impl From<u8> for ObjectType {
    fn from(value: u8) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

impl From<ObjectType> for u8 {
    fn from(value: ObjectType) -> Self {
        value as u8
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
    pub fn inc(&mut self) -> usize {
        match self {
            Self::None => 0,
            Self::String(value) => value.inc(),
            Self::Reference(value) => value.inc(),
            Self::Coroutine(value) => value.inc(),
        }
    }

    pub fn dec(&mut self) -> usize {
        match self {
            Self::None => 0,
            Self::String(value) => value.dec(),
            Self::Reference(value) => value.dec(),
            Self::Coroutine(value) => value.dec(),
        }
    }


    pub fn mark_reference(&mut self) {
        match self {
            Self::None | Self::Reference(..) | Self::String(..) | Self::Coroutine(..) => (),
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
