use std::{hash::{Hash, Hasher}};

use common::Value;
use rustc_hash::FxHasher;

use crate::{
    garbage::{GcSized}, Coroutine, String
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

#[derive(Default, Clone)]
pub enum Object {
    #[default]
    None,
    String(String),
    Coroutine(Coroutine<Value>),
}

impl GcSized for Object {
    fn size(&self) -> usize {
        match self {
            Self::None => 0,
            Self::String(value) => value.size(),
            Self::Coroutine(value) => value.size(),
        }
    }
}


// impl Object {
//     pub fn inc(&mut self) -> usize {
//         match self {
//             Self::None => 0,
//             Self::String(value) => value.inc(),
//             Self::Coroutine(value) => value.inc(),
//         }
//     }
//
//     pub fn dec(&mut self) -> usize {
//         match self {
//             Self::None => 0,
//             Self::String(value) => value.dec(),
//             Self::Coroutine(value) => value.dec(),
//         }
//     }
//
//
//     pub fn mark_reference(&mut self) {
//         match self {
//             Self::None | Self::String(..) | Self::Coroutine(..) => (),
//         }
//     }
//
// }

impl std::fmt::Display for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Object::String(value) => value.to_string(),
                _ => std::string::String::default(),
            }
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(value) => write!(f, "{:?}", value.to_string()),
            _ => write!(f, "Object"),
        }
    }
}
