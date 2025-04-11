use core::fmt::Display;
use libffi::{
    low::types::{double, pointer, sint64, uint8, void},
    raw::ffi_type as FFIType,
};

use crate::Value;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum Type {
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
    Object,
}

impl From<Type> for usize {
    fn from(value: Type) -> Self {
        (value as u8) as usize
    }
}

impl From<usize> for Type {
    fn from(value: usize) -> Self {
        match value {
            0 => Type::None,
            1 => Type::Bool,
            2 => Type::Integer,
            3 => Type::Float,
            4 => Type::String,
            5 => Type::Function,
            _ => Type::None,
        }
    }
}

impl From<&Value> for Type {
    fn from(value: &Value) -> Self {
        match value {
            Value::NONE => Type::None,
            Value::BOOLEAN(_) => Type::Bool,
            Value::INTEGER(_) => Type::Integer,
            Value::FLOAT(_) => Type::Float,
            Value::STR(_) | Value::STRING(_) => Type::String,
            Value::RANGE(_, _) => Type::Range,
            // ValueKind::FILE(_) => Type::
            Value::FUNCTION(_, _) => Type::Function,
            Value::FILE(_) | Value::RESOURCE(_) => Type::Resource,
            Value::POINTER(_) => Type::Pointer,
            Value::REFERENCE(_) => {
                todo!(
                    "Investigate how to transfer objects between C & Rust dynamically (if possible)"
                );
            }
            Value::FFI(_) => {
                unreachable!("FFI wrapping modules must not be converted to types")
            }
            Value::OBJECT(_) => Type::Object,
            Value::ITERATOR(_) => {
                unreachable!("Iterator how to");
            }
        }
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Type::None => "void",
                Type::Bool => "bool",
                Type::Integer => "int",
                Type::Float => "float",
                Type::String => "string",
                Type::Function => "func",
                Type::Range => "range",
                Type::Resource => "resource",
                Type::Pointer => "pointer",
                Type::Reference => "reference",
                Type::Object => "object",
            }
        )
    }
}

impl From<Type> for FFIType {
    fn from(value: Type) -> Self {
        match value {
            Type::None => unsafe { *Box::into_raw(Box::from(void)) },
            Type::Bool => unsafe { uint8 },
            Type::Integer => unsafe { sint64 },
            Type::Float => unsafe { double },
            Type::String | Type::Range => unsafe { pointer },
            Type::Resource | Type::Pointer => unsafe { pointer },
            _ => todo!("Handle other types"),
        }
    }
}
