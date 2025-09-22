use std::{
    fmt::{Debug, Display},
    hash::Hash,
};

use crate::{calculate_hash, garbage::GcSized};

#[derive(Clone)]
pub struct String(usize, u64, std::string::String);

impl String {
    pub fn length(&self) -> usize {
        self.0
    }

    pub fn hash(&self) -> u64 {
        self.1
    }

    pub fn as_str(&self) -> &str {
        self.2.as_str()
    }
}

impl GcSized for String {
    fn size(&self) -> usize {
        use std::mem::{size_of_val, size_of};

        size_of_val(&self.0) + size_of_val(&self.1) + size_of_val(&self.2) + (size_of::<char>() * self.2.len())
    }
}

impl From<std::string::String> for String {
    fn from(value: std::string::String) -> Self {
        Self(value.len(), calculate_hash(&value), value)
    }
}

impl From<&str> for String {
    fn from(value: &str) -> Self {
        Self::from(value.to_string().clone())
    }
}

impl Hash for String {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.1);
    }
}

impl Display for String {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.2)
    }
}

#[cfg(debug_assertions)]
impl Debug for String {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}' (0x{:0x})", self.2, self.1)
    }
}
