use std::{fmt::Display, hash::Hash};

use crate::{ObjectType, calculate_hash, garbage::GcSized};

#[derive(Clone)]
pub struct String {
    kind: ObjectType,
    length: usize,
    hash: u64,
    content: std::string::String,
}

impl String {
    pub fn length(&self) -> usize {
        self.length
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    pub fn as_str(&self) -> &str {
        self.content.as_str()
    }
}

impl GcSized for String {
    fn size(&self) -> usize {
        use std::mem::{size_of, size_of_val};

        size_of_val(&self.length)
            + size_of_val(&self.hash)
            + size_of_val(&self.content)
            + (size_of::<char>() * self.content.len())
    }
}

impl From<std::string::String> for String {
    fn from(value: std::string::String) -> Self {
        Self {
            kind: ObjectType::String,
            length: value.len(),
            hash: calculate_hash(&value),
            content: value,
        }
    }
}

impl From<&str> for String {
    fn from(value: &str) -> Self {
        Self::from(value.to_string().clone())
    }
}

impl Hash for String {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.length);
    }
}

impl Display for String {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for String {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}' (0x{:0x})", self.content, self.hash)
    }
}
