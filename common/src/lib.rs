use memory::object::Objects;
use native::Native;
use serde::{Deserialize, Serialize};

use std::{
    ffi::{CStr, CString, c_void},
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
    ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Neg, Not, Rem, Shl, Shr, Sub},
};

use program::data::Data;
use types::Kind;

pub mod error;
pub mod interner;
pub mod symbols;
// pub mod memory;
pub mod memory;
pub mod native;
pub mod opcodes;
pub mod program;
pub mod types;
pub mod vec_array;

#[inline(always)]
#[cold]
fn cold() {}

#[inline(always)]
#[must_use]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }

    b
}

#[inline(always)]
#[must_use]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }

    b
}

#[macro_export]
macro_rules! guarantee {
    ($($cond:expr)*) => {
        unsafe {
            std::hint::assert_unchecked($($cond)*);
        }
    };
}

#[inline]
pub fn calculate_hash<V: Hash>(value: &V) -> u64 {
    let mut hash = rustc_hash::FxHasher::default();
    value.hash(&mut hash);

    hash.finish()
}

#[derive(Default, Copy, Clone, Deserialize, Serialize)]
#[repr(u8)]
pub enum Value {
    #[default]
    NONE = 0,
    BOOLEAN(bool),
    INTEGER(i64),
    FLOAT(f64),
    STR(usize),
    FUNCTION(usize, usize),
    NATIVE(usize),
    // ARRAY(Key),
    RANGE(i64, i64),
    FILE(usize),
    REFERENCE(usize),
    RESOURCE(usize),
    #[serde(skip)]
    POINTER(*mut c_void),
    FFI(usize),
    #[serde(skip)]
    OBJECT(Objects),
    // ITERATOR(Objects, usize),
    TYPE(usize),
}

// impl Serialize for Value {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: serde::Serializer,
//     {
//         match self {
//             &Value::NONE => serializer.serialize_none(),
//             &Value::BOOLEAN(state) => serializer.serialize_bool(state),
//             &Value::INTEGER(value) => serializer.serialize_i64(value),
//             &Value::FLOAT(value) => serializer.serialize_f64(value),
//             &Value::FUNCTION(value) => {}
//         }
//     }
// }

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
            (Self::OBJECT(Objects::String(l)), Self::OBJECT(Objects::String(r))) => {
                l.as_ref().hash() == r.as_ref().hash()
            }
            (Self::OBJECT(Objects::Object(l)), Self::OBJECT(Objects::Object(r))) => {
                let l = calculate_hash(l.as_ref());
                let r = calculate_hash(r.as_ref());

                l == r
            }
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
            (Value::INTEGER(lhs), &Value::FLOAT(rhs)) => lhs.partial_cmp(&(rhs as i64)),
            (Value::FLOAT(lhs), &Value::INTEGER(rhs)) => lhs.partial_cmp(&(rhs as f64)),
            (Value::BOOLEAN(lhs), &Value::BOOLEAN(rhs)) => lhs.partial_cmp(&rhs),
            (Value::NONE, Value::NONE) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }
}

impl Add for &Value {
    type Output = Value;

    fn add(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs + rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Value::FLOAT(lhs + rhs),
            (&Value::INTEGER(lhs), &Value::FLOAT(rhs)) => Value::FLOAT(lhs as f64 + rhs),
            (&Value::FLOAT(lhs), &Value::INTEGER(rhs)) => Value::FLOAT(lhs + rhs as f64),
            _ => Value::NONE,
        }
    }
}

impl Sub for &Value {
    type Output = Value;

    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs - rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Value::FLOAT(lhs - rhs),
            (&Value::INTEGER(lhs), &Value::FLOAT(rhs)) => Value::FLOAT(lhs as f64 - rhs),
            (&Value::FLOAT(lhs), &Value::INTEGER(rhs)) => Value::FLOAT(lhs - rhs as f64),
            _ => Value::NONE,
        }
    }
}

impl Mul for &Value {
    type Output = Value;

    fn mul(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs * rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Value::FLOAT(lhs * rhs),
            (&Value::INTEGER(lhs), &Value::FLOAT(rhs)) => Value::FLOAT(lhs as f64 * rhs),
            (&Value::FLOAT(lhs), &Value::INTEGER(rhs)) => Value::FLOAT(lhs * rhs as f64),
            _ => Value::NONE,
        }
    }
}

impl Div for &Value {
    type Output = Value;

    fn div(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs / rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Value::FLOAT(lhs / rhs),
            (&Value::INTEGER(lhs), &Value::FLOAT(rhs)) => Value::FLOAT(lhs as f64 / rhs),
            (&Value::FLOAT(lhs), &Value::INTEGER(rhs)) => Value::FLOAT(lhs / rhs as f64),
            _ => Value::NONE,
        }
    }
}

impl Rem for &Value {
    type Output = Value;

    fn rem(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs % rhs),
            (Value::FLOAT(lhs), Value::FLOAT(rhs)) => Value::FLOAT(lhs % rhs),
            (&Value::INTEGER(lhs), &Value::FLOAT(rhs)) => Value::FLOAT(lhs as f64 % rhs),
            (&Value::FLOAT(lhs), &Value::INTEGER(rhs)) => Value::FLOAT(lhs % rhs as f64),
            _ => Value::NONE,
        }
    }
}

impl Shl for &Value {
    type Output = Value;

    fn shl(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs << rhs),
            _ => Value::NONE,
        }
    }
}

impl Shr for &Value {
    type Output = Value;

    fn shr(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs >> rhs),
            _ => Value::NONE,
        }
    }
}

impl BitAnd for &Value {
    type Output = Value;

    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs & rhs),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Value::BOOLEAN(lhs & rhs),
            _ => Value::NONE,
        }
    }
}

impl BitOr for &Value {
    type Output = Value;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs | rhs),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Value::BOOLEAN(lhs | rhs),
            _ => Value::NONE,
        }
    }
}

impl BitXor for &Value {
    type Output = Value;

    fn bitxor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::INTEGER(lhs), Value::INTEGER(rhs)) => Value::INTEGER(lhs ^ rhs),
            (Value::BOOLEAN(lhs), Value::BOOLEAN(rhs)) => Value::BOOLEAN(lhs ^ rhs),
            _ => Value::NONE,
        }
    }
}

impl Not for &Value {
    type Output = Value;

    fn not(self) -> Self::Output {
        match self {
            Value::INTEGER(lhs) => Value::INTEGER(!lhs),
            Value::BOOLEAN(lhs) => Value::BOOLEAN(!lhs),
            Value::NONE => Value::BOOLEAN(true),
            _ => Value::NONE,
        }
    }
}

impl Neg for &Value {
    type Output = Value;

    fn neg(self) -> Self::Output {
        match self {
            Value::INTEGER(rhs) => Value::INTEGER(-rhs),
            Value::FLOAT(rhs) => Value::FLOAT(-rhs),
            _ => Value::NONE,
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
            Value::FUNCTION(arity, l) => {
                "fn".hash(state);
                arity.hash(state);
                l.hash(state);
            }
            Value::RESOURCE(ptr) => {
                "res".hash(state);
                format!("{ptr:p}").hash(state);
            }
            Value::POINTER(ptr) => {
                "ptr".hash(state);
                format!("{ptr:p}").hash(state);
            }
            Value::REFERENCE(idx) => {
                "ref".hash(state);
                idx.hash(state);
            }
            Value::OBJECT(object) => {
                "o".hash(state);
                object.hash(state);
            }
            // Value::ITERATOR(o, n) => {
            //     "iter".hash(state);
            //     match o {
            //         Objects::Array(arr) => arr.hash(state),
            //         Objects::Object(obj) => obj.hash(state),
            //         Objects::String(val) => val.hash(state),
            //         Objects::None => 0.hash(state),
            //     }
            //     n.hash(state);
            // }
            Value::TYPE(n) => {
                "ty".hash(state);
                n.hash(state);
            }
            Value::NATIVE(func) => {
                "ext".hash(state);
                func.hash(state);
            }
            _ => {
                eprintln!("Value does not support hashing");

                state.write_u8(0);
            }
        }
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Value::INTEGER(int) => format!("{int}"),
                Value::FLOAT(f) => format!("{f:.?}"),
                Value::NONE => String::new(),
                Value::BOOLEAN(b) => format!("{b}"),
                Value::STR(s) => format!("{s}"),
                Value::OBJECT(Objects::String(s)) => format!("{}", s.as_ref().to_string()),
                value => format!("{value:?}"),
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
                Value::INTEGER(int) => format!("int({int})"),
                Value::FLOAT(f) => format!("float({f:.?})"),
                Value::NONE | Value::OBJECT(Objects::None) => String::from("void"),
                Value::BOOLEAN(b) => format!("bool({b})"),
                Value::STR(s) => format!("string({s})"),
                Value::FUNCTION(_, symbol) => format!("fn({symbol})"),
                // ValueKind::ARRAY(a) => format!("arr({})", a),
                Value::RANGE(start, end) => format!("range({start}, {end})"),
                Value::FILE(fd) => format!("file({fd})"),
                Value::RESOURCE(_) => "resuorce".to_string(),
                Value::POINTER(n) => format!("pointer({n:p})"),
                Value::FFI(id) => format!("dynamic({id})"),
                Value::REFERENCE(idx) => format!("ref({idx:?})"),
                Value::NATIVE(ext) => format!("ext(0x{:0x})", std::ptr::addr_of!(ext) as u64),
                Value::OBJECT(Objects::Array(obj)) =>
                    format!("array(0x{:0x})", std::ptr::addr_of!(obj) as u64),
                Value::OBJECT(Objects::Object(obj)) =>
                    format!("obj(0x{:0x})", std::ptr::addr_of!(obj) as u64),
                Value::OBJECT(Objects::String(obj)) =>
                    format!("str(0x{:0x})", std::ptr::addr_of!(obj) as u64),
                Value::OBJECT(Objects::Iterator(obj)) => format!(
                    "iter(0x{:0x}, {})",
                    std::ptr::addr_of!(obj) as u64,
                    obj.as_ref().tell()
                ),
                Value::OBJECT(Objects::Coroutine(coro)) =>
                    format!("coro(0x{:0x})", std::ptr::addr_of!(coro) as u64,),
                // Value::ITERATOR(obj, cursor) => format!(
                //     "iter(0x{:0x}, {cursor})",
                //     match obj {
                //         Objects::Array(arr) => std::ptr::addr_of!(*arr.as_ref()) as u64,
                //         Objects::Object(obj) => std::ptr::addr_of!(*obj.as_ref()) as u64,
                //         Objects::String(val) => std::ptr::addr_of!(*val.as_ref()) as u64,
                //         Objects::None => 0,
                //     }
                // ),
                Value::TYPE(n) => format!("type({n})"),
            }
        )
    }
}

// @TODO:: Handle to & from raw pointer by wrapping a pointer in a struct that can actually do the
//  drop

impl Value {
    #[must_use]
    pub fn try_into_raw(&self) -> Option<*mut c_void> {
        Some(
            (match self {
                Value::NONE => Box::into_raw(Box::new(std::ptr::null::<c_void>())).cast::<c_void>(),
                Value::BOOLEAN(state) => Box::into_raw(Box::new(u8::from(*state))).cast::<c_void>(),
                Value::INTEGER(number) => Box::into_raw(Box::new(*number)).cast::<c_void>(),
                Value::FLOAT(number) => Box::into_raw(Box::new(*number)).cast::<c_void>(),
                _ => return None,
            })
            .cast::<c_void>(),
        )
    }

    pub fn ptr(&self, data: &mut Data) -> Option<ValuePtr> {
        match self {
            Value::STR(index) => {
                let string = data.string(*index);
                if let Ok(value) = CString::new(string.as_str()) {
                    Some(ValuePtr {
                        ptr: Box::into_raw(Box::new(value)) as *const c_void,
                        kind: Kind::from(self),
                    })
                } else {
                    None
                }
            }
            // Value::RESOURCE(ptr) => data.pointer(*ptr).map(|ptr| ValuePtr {
            //     ptr,
            //     kind: Type::Resource,
            // }),
            Value::POINTER(ptr) => Some(ValuePtr {
                ptr: Box::into_raw(Box::new(ptr)) as *const _,
                kind: Kind::Pointer,
            }),

            _ => self.try_into_raw().map(|ptr| ValuePtr {
                ptr,
                kind: Kind::from(self),
            }),
        }
    }

    pub fn from_ptr_and_type(ptr: *mut c_void, kind: Kind, data: &mut Data) -> Self {
        match kind {
            Kind::None => Self::default(),
            Kind::Bool => Self::from(ptr.cast::<u8>() as u8 != 0),
            Kind::Integer => Self::from(ptr.cast::<i64>() as i64),
            Kind::Float => Self::from(f64::from_bits(ptr.cast::<u64>() as u64)),
            Kind::String => {
                let string = unsafe {
                    CStr::from_ptr(ptr as *const i8)
                        .to_string_lossy()
                        .into_owned()
                };
                Self::string(data.add_string(string))
            }
            Kind::Resource | Kind::Pointer => Self::pointer(ptr),
            _ => {
                eprintln!("Unable to create value from pointer & type");

                Self::default()
            }
        }
    }
    #[must_use]
    pub fn string(identifier: usize) -> Value {
        Self::STR(identifier)
    }

    #[must_use]
    pub fn resource(identifier: usize) -> Value {
        Self::RESOURCE(identifier)
    }

    #[must_use]
    pub fn pointer(identifier: *mut c_void) -> Value {
        Self::POINTER(identifier)
    }
}

#[derive(Debug)]
pub struct ValuePtr {
    ptr: *const c_void,
    kind: Kind,
}

impl ValuePtr {
    #[must_use]
    pub fn new(ptr: *const c_void, kind: Kind) -> Self {
        Self { ptr, kind }
    }
    #[must_use]
    pub fn ptr<R>(&self) -> *const R {
        self.ptr.cast::<R>()
    }

    pub fn ptr_mut<R>(&mut self) -> *mut R {
        self.ptr as *mut R
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        self.kind
    }
}

impl Drop for ValuePtr {
    fn drop(&mut self) {
        // dbg!(unsafe { Box::from_raw(self.ptr as *mut &str) });
        match self.kind {
            Kind::None => unsafe { drop(Box::from_raw(self.ptr.cast_mut())) },
            Kind::Bool => unsafe { drop(Box::from_raw(self.ptr as *mut u8)) },
            Kind::Integer => unsafe { drop(Box::from_raw(self.ptr as *mut isize)) },
            Kind::Float => unsafe { drop(Box::from_raw(self.ptr as *mut f64)) },
            Kind::String => unsafe { drop(Box::from_raw(self.ptr as *mut CString)) },
            Kind::Resource => unsafe { drop(Box::from_raw(self.ptr.cast_mut())) },
            Kind::Pointer => unsafe { drop(Box::from_raw(self.ptr as *mut *mut c_void)) },
            _ => (),
        }
    }
}

impl From<Value> for Kind {
    fn from(val: Value) -> Self {
        match val {
            Value::NONE => Kind::None,
            Value::BOOLEAN(_) => Kind::Bool,
            Value::INTEGER(_) => Kind::Integer,
            Value::FLOAT(_) => Kind::Float,
            Value::STR(_) => Kind::String,
            Value::FUNCTION(_, _) => Kind::Function,
            Value::RESOURCE(_) => Kind::Resource,
            Value::POINTER(_) => Kind::Pointer,
            Value::OBJECT(Objects::Coroutine(_)) => Kind::Coroutine(0),
            _ => Kind::None,
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
