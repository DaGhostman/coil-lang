use generational_arena::Index;
use memory2::object::Objects;
use std::{
    ffi::{CStr, CString, c_void},
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
    ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Neg, Not, Rem, Shl, Shr, Sub},
};

use program::data::Data;
use types::Type;

pub mod error;
pub mod interner;
pub mod symbols;
// pub mod memory;
pub mod hasher;
pub mod memory2;
pub mod opcodes;
pub mod program;
pub mod types;

#[derive(Default, Copy, Clone)]
pub enum Value {
    #[default]
    NONE,
    BOOLEAN(bool),
    INTEGER(i64),
    FLOAT(f64),
    STR(usize),
    FUNCTION(usize, usize),
    // ARRAY(Key),
    RANGE(i64, i64),
    FILE(usize),
    REFERENCE(Index),
    RESOURCE(usize),
    POINTER(usize),
    FFI(usize),
    STRING(Objects),
    OBJECT(Objects),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NONE, Self::NONE) => true,
            (Self::BOOLEAN(l), Self::BOOLEAN(r)) => l == r,
            (Self::INTEGER(l), Self::INTEGER(r)) => l == r,
            (Self::FLOAT(l), Self::FLOAT(r)) => l == r,
            (Self::STR(l), Self::STR(r)) => l == r,
            (Self::FUNCTION(l, la), Self::FUNCTION(r, ra)) => l == r && la == ra,
            (Self::RESOURCE(l), Self::RESOURCE(r)) => l == r,
            (Self::POINTER(l), Self::POINTER(r)) => l == r,
            (Self::FFI(l), Self::FFI(r)) => l == r,
            _ => false,
        }
    }
}

impl Eq for Value {}
unsafe impl Send for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => lhs.partial_cmp(rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => lhs.partial_cmp(rhs),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => lhs.partial_cmp(&(*rhs as i64)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => lhs.partial_cmp(&(*rhs as f64)),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => lhs.partial_cmp(rhs),
            (Value::NONE, Value::NONE) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }
}

impl Add for Value {
    type Output = Option<Value>;

    fn add(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs + rhs)),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs + rhs)),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs as f64 + rhs)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => Some(Value::FLOAT(lhs + rhs as f64)),
            _ => None,
        }
    }
}

impl Sub for Value {
    type Output = Option<Value>;

    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs - rhs)),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs - rhs)),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs as f64 - rhs)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => Some(Value::FLOAT(lhs - rhs as f64)),
            _ => None,
        }
    }
}

impl Mul for Value {
    type Output = Option<Value>;

    fn mul(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs * rhs)),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs * rhs)),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs as f64 * rhs)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => Some(Value::FLOAT(lhs * rhs as f64)),
            _ => None,
        }
    }
}

impl Div for Value {
    type Output = Option<Value>;

    fn div(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs / rhs)),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs / rhs)),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs as f64 / rhs)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => Some(Value::FLOAT(lhs / rhs as f64)),
            _ => None,
        }
    }
}

impl Rem for Value {
    type Output = Option<Value>;

    fn rem(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs % rhs)),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs % rhs)),
            (Value::INTEGER(lhs), Value::FLOAT(rhs)) => Some(Value::FLOAT(lhs as f64 % rhs)),
            (Value::FLOAT(lhs), Value::INTEGER(rhs)) => Some(Value::FLOAT(lhs % rhs as f64)),
            _ => None,
        }
    }
}

impl Shl for Value {
    type Output = Option<Value>;

    fn shl(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs << rhs)),
            _ => None,
        }
    }
}

impl Shr for Value {
    type Output = Option<Value>;

    fn shr(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs >> rhs)),
            _ => None,
        }
    }
}

impl BitAnd for Value {
    type Output = Option<Value>;

    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs & rhs)),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Some(Value::BOOLEAN(lhs & rhs)),
            _ => None,
        }
    }
}

impl BitOr for Value {
    type Output = Option<Value>;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs | rhs)),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Some(Value::BOOLEAN(lhs | rhs)),
            _ => None,
        }
    }
}

impl BitXor for Value {
    type Output = Option<Value>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Some(Value::INTEGER(lhs ^ rhs)),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Some(Value::BOOLEAN(lhs ^ rhs)),
            _ => None,
        }
    }
}

impl Not for Value {
    type Output = Option<Value>;

    fn not(self) -> Self::Output {
        match self {
            Value::INTEGER(lhs) => Some(Value::INTEGER(!lhs)),
            Value::BOOLEAN(lhs) => Some(Value::BOOLEAN(!lhs)),
            _ => None,
        }
    }
}

impl Neg for Value {
    type Output = Option<Value>;

    fn neg(self) -> Self::Output {
        match self {
            Value::INTEGER(rhs) => Some(Value::INTEGER(-rhs)),
            Value::FLOAT(rhs) => Some(Value::FLOAT(-rhs)),
            _ => None,
        }
    }
}

impl Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Value::NONE => {
                state.write_u8(0);
            }
            Value::BOOLEAN(value) => {
                "b".hash(state);
                value.hash(state);
            }
            Value::INTEGER(value) => {
                "i".hash(state);
                value.hash(state);
            }
            Value::FLOAT(value) => {
                "f".hash(state);
                value.to_bits().hash(state);
            }
            Value::STR(value) => {
                "s".hash(state);
                value.hash(state);
            }
            // ValueKind::ARRAY(value) => {
            //     "a".hash(state);
            //     value.hash(state);
            // }
            Value::RANGE(start, end) => {
                "r".hash(state);
                start.hash(state);
                end.hash(state);
            }
            Value::FILE(fd) => {
                "fs".hash(state);
                fd.hash(state);
            }
            Value::FUNCTION(arity, _) => {
                "fn".hash(state);
                arity.hash(state);
            }
            Value::RESOURCE(ptr) | Value::POINTER(ptr) => {
                "res".hash(state);
                format!("{:p}", ptr).hash(state);
            }
            Value::REFERENCE(idx) => {
                "ref".hash(state);
                idx.hash(state);
            }
            Value::OBJECT(object) => {
                "o".hash(state);
                object.hash(state);
            }
            _ => panic!("No support"),
        }
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Value::INTEGER(int) => format!("{}", int),
                Value::FLOAT(f) => format!("{:.?}", f),
                Value::NONE => String::from(""),
                Value::BOOLEAN(b) => format!("{}", b),
                Value::STR(s) => format!("{}", s),
                value => format!("{:?}", value),
            }
        )
    }
}

impl Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Value::INTEGER(int) => format!("int({})", int),
                Value::FLOAT(f) => format!("float({:.?})", f),
                Value::NONE => String::from("void"),
                Value::BOOLEAN(b) => format!("bool({})", b),
                Value::STR(s) => format!("string({})", s),
                Value::STRING(s) => format!("string({})", s),
                Value::FUNCTION(_, symbol) => format!("fn({})", symbol),
                // ValueKind::ARRAY(a) => format!("arr({})", a),
                Value::RANGE(start, end) => format!("range({}, {})", start, end),
                Value::FILE(fd) => format!("file({})", fd),
                Value::RESOURCE(_) => "resuorce".to_string(),
                Value::POINTER(n) => format!("pointer({})", n),
                Value::FFI(id) => format!("dynamic({})", id),
                Value::REFERENCE(idx) => format!("ref({:?})", idx),
                Value::OBJECT(obj) => format!("obj({})", std::ptr::addr_of!(obj) as u64),
            }
        )
    }
}

// @TODO:: Handle to & from raw pointer by wrapping a pointer in a struct that can actually do the
//  drop

impl Value {
    pub fn try_into_raw(&self) -> Option<*mut c_void> {
        Some(match self {
            Value::NONE => Box::into_raw(Box::new(std::ptr::null::<c_void>())) as *mut c_void,
            Value::BOOLEAN(state) => Box::into_raw(Box::new(*state as u8)) as *mut c_void,
            Value::INTEGER(number) => Box::into_raw(Box::new(*number)) as *mut c_void,
            Value::FLOAT(number) => Box::into_raw(Box::new(*number)) as *mut c_void,
            _ => return None,
        } as *mut c_void)
    }

    pub fn ptr(&self, data: &mut Data) -> Option<ValuePtr> {
        match self {
            Value::STR(index) => {
                let string = data.string(*index);
                if let Ok(value) = CString::new(string.as_str()) {
                    Some(ValuePtr {
                        ptr: Box::into_raw(Box::new(value)) as *const c_void,
                        kind: Type::from(self),
                    })
                } else {
                    None
                }
            }
            Value::RESOURCE(ptr) => data.pointer(*ptr).map(|ptr| ValuePtr {
                ptr,
                kind: Type::Resource,
            }),
            Value::POINTER(ptr) => data.pointer(*ptr).map(|ptr| ValuePtr {
                ptr: Box::into_raw(Box::new(ptr)) as *const _,
                kind: Type::Pointer,
            }),

            _ => self.try_into_raw().map(|ptr| ValuePtr {
                ptr,
                kind: Type::from(self),
            }),
        }
    }

    pub fn from_ptr_and_type(ptr: *mut c_void, kind: Type, data: &mut Data) -> Self {
        match kind {
            Type::None => Self::default(),
            Type::Bool => Self::from((ptr as *mut u8) as u8 != 0),
            Type::Integer => Self::from((ptr as *mut i64) as i64),
            Type::Float => Self::from(f64::from_bits((ptr as *mut u64) as u64)),
            Type::String => {
                let string = unsafe {
                    CStr::from_ptr(ptr as *const i8)
                        .to_string_lossy()
                        .into_owned()
                };
                Self::string(data.add_string(string))
            }
            Type::Resource => Self::resource(data.add_pointer(ptr)),
            Type::Pointer => Self::pointer(data.add_pointer(ptr)),
            _ => todo!(),
        }
    }
    pub fn string(identifier: usize) -> Value {
        Self::STR(identifier)
    }

    pub fn resource(identifier: usize) -> Value {
        Self::RESOURCE(identifier)
    }

    pub fn pointer(identifier: usize) -> Value {
        Self::POINTER(identifier)
    }
}

#[derive(Debug)]
pub struct ValuePtr {
    ptr: *const c_void,
    kind: Type,
}

impl ValuePtr {
    pub fn new(ptr: *const c_void, kind: Type) -> Self {
        Self { ptr, kind }
    }
    pub fn ptr<R>(&self) -> *const R {
        self.ptr as *const R
    }

    pub fn ptr_mut<R>(&mut self) -> *mut R {
        self.ptr as *mut R
    }

    pub fn kind(&self) -> Type {
        self.kind
    }
}

impl Drop for ValuePtr {
    fn drop(&mut self) {
        // dbg!(unsafe { Box::from_raw(self.ptr as *mut &str) });
        match self.kind {
            Type::None => unsafe { drop(Box::from_raw(self.ptr as *mut c_void)) },
            Type::Bool => unsafe { drop(Box::from_raw(self.ptr as *mut u8)) },
            Type::Integer => unsafe { drop(Box::from_raw(self.ptr as *mut isize)) },
            Type::Float => unsafe { drop(Box::from_raw(self.ptr as *mut f64)) },
            Type::String => unsafe { drop(Box::from_raw(self.ptr as *mut CString)) },
            Type::Resource => unsafe { drop(Box::from_raw(self.ptr as *mut c_void)) },
            Type::Pointer => unsafe { drop(Box::from_raw(self.ptr as *mut *mut c_void)) },
            _ => (),
        };
    }
}

impl From<Value> for Type {
    fn from(val: Value) -> Self {
        match val {
            Value::NONE => Type::None,
            Value::BOOLEAN(_) => Type::Bool,
            Value::INTEGER(_) => Type::Integer,
            Value::FLOAT(_) => Type::Float,
            Value::STR(_) => Type::String,
            Value::FUNCTION(_, _) => Type::Function,
            Value::RESOURCE(_) => Type::Resource,
            Value::POINTER(_) => Type::Pointer,
            _ => todo!("Handle remaining value to type conversion cases"),
        }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::INTEGER(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::FLOAT(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::BOOLEAN(value)
    }
}
