use core::fmt::Display;
use libffi::{
    low::types::{double, pointer, sint64, uint8, void},
    raw::ffi_type as FFIType,
};
use std::fmt::Debug;

use crate::{Value, memory::object::Objects, program::data::Data};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    #[default]
    None,
    Bool,
    Integer,
    Float,
    String,
    Range(usize),
    Function,
    Resource,
    Pointer,
    Reference,
    Object(usize),
    List(usize),
    Generic(usize, usize),
    Result,
    Error,
    Union,
    Intersection,
    Type,
    Wildcard,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Type {
    own: Kind,
    // This should be handled in such a case that upon reaching the limit, the
    // rest of the params will be unchecked
    params: [usize; 32],
    arguments: [usize; 32],
    return_type: usize,
    has_return: bool,
    arity: usize,
    placeholders: usize,
}

impl Type {
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            own: kind,
            params: [0; 32],
            arguments: [0; 32],
            arity: 0,
            placeholders: 0,
            has_return: false,
            return_type: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.arity
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        self.own
    }

    pub fn add(&mut self, kind: usize) {
        if self.arity >= 32 {
            return;
        }

        self.params[self.arity] = kind;
        self.arity += 1;
    }

    #[must_use]
    pub fn get(&self, position: usize) -> usize {
        self.params[position]
    }

    pub fn set(&mut self, position: usize, kind: usize) {
        self.arity = self.arity.max(position.max(1));

        self.params[position] = kind;

        self.arity = self.arity.max(position);
    }

    pub fn add_argument(&mut self, r#type: usize) -> usize {
        let key = self.placeholders;
        self.placeholders += 1;
        self.arguments[key] = r#type;

        key
    }

    #[must_use]
    pub fn get_argument(&self, idx: usize) -> usize {
        assert!((idx < 32), "Too much arguments");

        self.arguments[idx]
    }

    #[must_use]
    pub fn arguments(&self) -> &[usize] {
        &self.arguments[..self.placeholders]
    }

    pub fn set_return(&mut self, kind: usize) {
        self.has_return = true;
        self.return_type = kind;
    }

    #[must_use]
    pub fn has_return_type(&self) -> bool {
        self.has_return
    }

    #[must_use]
    pub fn returns(&self) -> usize {
        self.return_type
    }

    #[must_use]
    pub fn integer() -> Self {
        Type::new(Kind::Integer)
    }

    #[must_use]
    pub fn float() -> Self {
        Type::new(Kind::Float)
    }

    #[must_use]
    pub fn bool() -> Self {
        Type::new(Kind::Bool)
    }

    #[must_use]
    pub fn string() -> Self {
        Type::new(Kind::String)
    }

    #[must_use]
    pub fn object(id: usize) -> Self {
        Type::new(Kind::Object(id))
    }

    #[must_use]
    pub fn array(n: usize) -> Self {
        Type::new(Kind::List(n))
    }

    #[must_use]
    pub fn any() -> Self {
        Type::new(Kind::Wildcard)
    }

    #[must_use]
    pub fn void() -> Self {
        Type::new(Kind::None)
    }

    #[must_use]
    pub fn function() -> Self {
        Type::new(Kind::Function)
    }

    #[must_use]
    pub fn output(&self, data: &Data) -> String {
        match self.own {
            Kind::Result => format!(
                "Result<{}, {}>",
                data.get_type(self.get(0)).output(data),
                data.get_type(self.get(1)).output(data)
            ),
            Kind::Union => {
                let mut fmt = String::new();
                for idx in 0..self.arity {
                    fmt = format!("{} | {}", fmt, data.get_type(self.get(idx)).output(data));
                }

                fmt.trim_start_matches(" | ").to_string()
            }
            Kind::Intersection => {
                let mut fmt = String::new();
                for idx in 0..self.arity {
                    fmt = format!("{} & {}", fmt, data.get_type(self.get(idx)).output(data));
                }

                fmt.trim_start_matches(" & ").to_string()
            }
            Kind::Object(n) => {
                format!(
                    "{}({}){}",
                    self.own,
                    data.symbol_name(n),
                    if self.placeholders > 0 {
                        format!(
                            "<{}>",
                            self.arguments[..self.placeholders]
                                .iter()
                                .map(|t| data.get_type(*t).output(data))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        String::new()
                    }
                )
            }
            Kind::Generic(name, constraint) => {
                format!(
                    "${}{}",
                    data.symbol_name(name),
                    if constraint != 0 {
                        format!(": {}", data.get_type(constraint).output(data))
                    } else {
                        String::new()
                    }
                )
            }
            Kind::Wildcard => "%".to_string(),
            _ => {
                let mut fmt = format!("{}", self.own);
                if self.arity > 0 {
                    fmt = format!(
                        "{}({})",
                        fmt,
                        self.params[..self.arity]
                            .iter()
                            .map(|t| data.get_type(*t).output(data))
                            .collect::<Vec<String>>()
                            .join(", ")
                    );
                }

                if self.has_return {
                    fmt = format!(
                        "{} -> {}",
                        fmt,
                        data.get_type(self.return_type).output(data)
                    );
                }

                fmt
            }
            _ => format!("{:?}", self.kind()),
        }
    }

    #[must_use]
    pub fn matches(&self, other: Self) -> bool {
        false
    }

    #[must_use]
    pub fn intersect(&self, lhs: Self, rhs: Self) -> bool {
        lhs.matches(rhs) && rhs.matches(lhs)
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
            // Value::RANGE(_, _) => Kind::Range,
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
            Value::TYPE(_) => Kind::Type,
            _ => Kind::Wildcard,
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
                Kind::Range(_) => "range",
                Kind::Resource => "resource",
                Kind::Pointer => "pointer",
                Kind::Reference => "reference",
                Kind::Object(_) => "object",
                Kind::List(_) => "array",
                Kind::Result => "result",
                Kind::Error => "error",
                Kind::Intersection => "intersect",
                Kind::Union => "union",
                Kind::Generic(..) => "generic",
                Kind::Wildcard => "%",
                Kind::Type => "@type",
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
            Kind::String | Kind::Range(_) => unsafe { pointer },
            Kind::Resource | Kind::Pointer => unsafe { pointer },
            _ => todo!("Handle other types"),
        }
    }
}
