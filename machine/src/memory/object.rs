use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::{
    String,
    garbage::{Collectable, GcSized},
};

pub fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

#[derive(Default, Copy, Clone)]
pub enum Object {
    #[default]
    None,
    String(Collectable<String>),
}

impl GcSized for Object {
    fn size(&self) -> usize {
        match self {
            Self::None => 0,
            Self::String(s) => s.size(),
        }
    }
}

impl Object {
    pub fn mark(&mut self, grey: &mut Vec<Self>) {
        let marked = match self {
            Self::None => false,
            Self::String(value) => value.mark(),
        };

        if marked {
            grey.push(*self)
        }
    }

    pub fn unmark(&mut self) {
        match self {
            Self::None => (),
            Self::String(value) => value.unmark(),
        }
    }

    pub fn is_marked(&self) -> bool {
        match self {
            Self::None => true,
            Self::String(value) => value.is_marked(),
        }
    }

    pub fn mark_reference(&self, _: &mut Vec<Self>) {
        match self {
            Self::None | Self::String(_) => (),
        }
    }

    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::None => None,
            Self::String(value) => value.get_next(),
        }
    }

    pub fn set_next(&mut self, next: Option<Object>) {
        match self {
            Self::String(value) => value.set_next(next),
            _ => (),
        }
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "Object::None"),
            Self::String(value) => write!(f, "{:?}", value),
        }
    }
}
