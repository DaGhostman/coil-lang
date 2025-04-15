use core::fmt::Display;
use libffi::{
    low::types::{double, pointer, sint64, uint8, void},
    raw::ffi_type as FFIType,
};
use std::fmt::Debug;

use crate::{Value, memory::object::Objects, vec_array::VecArray};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    #[default]
    None,
    Bool,
    Integer,
    Float,
    String,
    Range,
    Function,
    Resource,
    Pointer,
    Reference,
    Object(usize),
    List(usize),
}

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct Type {
    own: Kind,
    // This should be handled in such a case that upon reaching the limit, the
    // rest of the params will be unchecked
    params: [Kind; 32],
    return_type: Kind,
    has_return: bool,
    counter: usize,
}

impl Type {
    pub fn new(kind: Kind) -> Self {
        Self {
            own: kind,
            params: [Kind::default(); 32],
            counter: 0,
            has_return: false,
            return_type: Kind::default(),
        }
    }

    pub fn kind(&self) -> Kind {
        self.own
    }

    pub fn add(&mut self, kind: Kind) {
        if self.counter >= 32 {
            return;
        }

        self.params[self.counter] = kind;
        self.counter += 1;
    }

    pub fn get(&self, position: usize) -> Kind {
        self.params[position]
    }

    pub fn set(&mut self, position: usize, kind: Kind) {
        self.params[position] = kind;

        self.counter = self.counter.max(position);
    }

    pub fn set_return(&mut self, kind: Kind) {
        self.has_return = true;
        self.return_type = kind;
    }

    pub fn has_return_type(&self) -> bool {
        self.has_return
    }

    pub fn returns(&self) -> Kind {
        self.return_type
    }
}

impl Debug for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut fmt = format!("{}", self.own);
        if self.counter > 0 {
            fmt = format!(
                "{}({})",
                fmt,
                self.params[..self.counter]
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<String>>()
                    .join(", ")
                    .to_string()
            )
        }

        if self.has_return {
            fmt = format!("{} -> {}", fmt, self.return_type);
        }
        write!(f, "{}", fmt)
    }
}

impl From<&Value> for Kind {
    fn from(value: &Value) -> Self {
        match value {
            Value::OBJECT(Objects::None) | Value::NONE => Kind::None,
            Value::BOOLEAN(_) => Kind::Bool,
            Value::INTEGER(_) => Kind::Integer,
            Value::FLOAT(_) => Kind::Float,
            Value::OBJECT(Objects::String(_)) | Value::STR(_) | Value::STRING(_) => Kind::String,
            Value::RANGE(_, _) => Kind::Range,
            // ValueKind::FILE(_) => Type::
            Value::FUNCTION(_, _) => Kind::Function,
            Value::FILE(_) | Value::RESOURCE(_) => Kind::Resource,
            Value::POINTER(_) => Kind::Pointer,
            Value::REFERENCE(_) => {
                todo!(
                    "Investigate how to transfer objects between C & Rust dynamically (if possible)"
                );
            }
            Value::FFI(_) => {
                unreachable!("FFI wrapping modules must not be converted to types")
            }
            Value::OBJECT(Objects::Object(o)) => Kind::Object(o.as_ref().name()),
            Value::OBJECT(Objects::Array(a)) => Kind::List(a.as_ref().len()),
            Value::ITERATOR(_) => {
                unreachable!("Iterator how to");
            }
        }
    }
}

impl Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Kind::None => "void",
                Kind::Bool => "bool",
                Kind::Integer => "int",
                Kind::Float => "float",
                Kind::String => "string",
                Kind::Function => "func",
                Kind::Range => "range",
                Kind::Resource => "resource",
                Kind::Pointer => "pointer",
                Kind::Reference => "reference",
                Kind::Object(_) => "object",
                Kind::List(_) => "array",
            }
        )
    }
}

impl From<Kind> for FFIType {
    fn from(value: Kind) -> Self {
        match value {
            Kind::None => unsafe { *Box::into_raw(Box::from(void)) },
            Kind::Bool => unsafe { uint8 },
            Kind::Integer => unsafe { sint64 },
            Kind::Float => unsafe { double },
            Kind::String | Kind::Range => unsafe { pointer },
            Kind::Resource | Kind::Pointer => unsafe { pointer },
            _ => todo!("Handle other types"),
        }
    }
}
