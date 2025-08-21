use std::{
    fmt::{Debug, Display},
    num::NonZero,
    ptr::NonNull,
};

use crate::{promise, unlikely};

#[repr(u8)]
#[derive(Debug, PartialEq)]
pub enum Type {
    Null,
    Integer,
    Float,
    Bool,
    String,
    Object,
}
// Pointer tagging from https://www.dannyvankooten.com/blog/2022/rewriting-interpreter-rust/
#[derive(Default, Copy, Clone)]
pub struct Value(*mut u8);

impl<'a> Value {
    /// Creates a new object from the value (or address)
    /// with the given type mask applied
    fn with_type(raw: *mut u8, t: Type) -> Self {
        Self((raw as usize | t as usize) as _)
    }

    /// Retrieve the type of the value
    pub fn get_type(self) -> Type {
        unsafe { std::mem::transmute((self.0 as usize & 0b111) as u8) }
    }
}

impl<'a> Value {
    /// Create a new integer value
    pub fn int(value: i64) -> Self {
        promise!(
            (((value as u64) << 3) >> 3) == value as u64,
            "There is data loss when shifting int"
        );

        Self::with_type((value << 3) as _, Type::Integer)
    }

    /// Create a new float value
    pub fn float(value: f64) -> Self {
        Self::with_type(((value.to_bits() >> 3) << 3) as _, Type::Float)
    }

    /// Create a new boolean value
    pub fn bool(value: bool) -> Self {
        promise!(
            (((value as u64) << 3) >> 3) == value as u64,
            "There is data loss when shifting bool"
        );

        Self::with_type(((value as u64) << 3) as _, Type::Bool)
    }

    /// Create a new string value, based on a non-null pointer.
    /// The idea behind this method is to have it used along with
    /// a garbage collector.
    /// ```
    /// use std::ptr::NonNull;
    ///
    /// let string = Box::new("Hello, World!".to_string());
    /// let ptr = NonNull::from(Box::leak(string));
    ///
    /// assert_eq!(
    ///     unsafe { *Box::from_raw(ptr.as_ptr()) },
    ///     "Hello, World!".to_string()
    /// );
    ///
    /// ```
    pub fn string<T>(value: NonNull<T>) -> Self {
        let ptr = value.as_ptr() as u64;

        promise!(((ptr << 3) >> 3) == ptr as u64);

        Self::with_type((ptr << 3) as _, Type::String)
    }

    /// Create a new string value, based on a non-null pointer.
    /// The idea behind this method is to have it used along with
    /// a garbage collector.
    /// ```
    /// use std::ptr::NonNull;
    ///
    /// let string = Box::new("Hello, World!".to_string());
    /// let ptr = NonNull::from(Box::leak(string));
    ///
    /// assert_eq!(
    ///     unsafe { *Box::from_raw(ptr.as_ptr()) },
    ///     "Hello, World!".to_string()
    /// );
    ///
    /// ```
    pub fn object<T>(value: NonNull<T>) -> Self {
        let ptr = value.as_ptr() as u64;

        promise!(((ptr << 3) >> 3) == ptr as u64);

        Self::with_type((ptr << 3) as _, Type::Object)
    }

    /// Replace the current object with a newly provided one
    pub fn replace(&mut self, value: u64) {
        promise!(((value << 3) >> 3) == value);

        *self = Self::with_type((value << 3) as _, self.get_type());
    }
}

impl<'a> Value {
    /// Casts the internal pointer value to i64
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::int(42).as_int(), 42);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_int(self) -> i64 {
        self.0 as i64 >> 3
    }

    /// Casts the internal pointer value to bool
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::bool(true).as_bool(), true);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_bool(self) -> bool {
        if (self.0 as i64 >> 3) == 0 {
            false
        } else {
            true
        }
    }

    /// Casts the internal pointer value to f64
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert!(1.2 - Value::float(1.2).as_float() < 0.0000_0000_0000_1);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_float(self) -> f64 {
        f64::from_bits(((self.0 as u64 >> 3) << 3) as u64)
    }

    pub fn as_ptr<T>(self) -> NonNull<T> {
        NonNull::without_provenance(
            NonZero::new(self.raw() as usize).expect("Invalid pointer address"),
        )
    }

    /// Casts the internal pointer value to f64
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::int(42).raw(), 42);
    /// assert_eq!(Value::bool(true).raw(), 1);
    /// assert!(1.2_f64 - f64::from_bits(Value::float(1.2).raw()) < 0.0000_0000_1);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn raw(self) -> u64 {
        if unlikely(self.get_type() == Type::Float) {
            return self.0 as u64;
        }

        self.0 as u64 >> 3
    }
}

// impl<'a> Object {
//     fn as_ptr(self) -> *mut u8 {
//         (self.0 as usize & !0b111) as _
//     }
//
//      fn get<T>(self) -> &'a T {
//          unsafe { &*(self.as_ptr() as *const T) }
//      }
// }

impl Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.get_type() {
                Type::Null => String::new(),
                Type::Integer | Type::Bool => format!("{}", self.as_int() as i64),
                Type::Float => format!("{:.?}", self.as_float() as f64),
                Type::Object | Type::String => format!("obj(0x{})", self.raw()),
                _ => unreachable!("Unknown value type"),
            }
        )
    }
}

#[cfg(debug_assertions)]
impl Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.get_type() {
                Type::Null => "null".to_string(),
                Type::Integer => format!("int({})", self),
                Type::Float => format!("float({:.?})", self.as_float()),
                Type::Bool => format!("bool({})", self),
                Type::Object | Type::String => format!("0x{:016x}", self.raw()),
                _ => unreachable!("Unknown value type"),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{Type, Value};

    #[test]
    fn ptr_tagging() {
        const MIN_FLOAT: f64 = f64::from_bits((f64::MIN.to_bits() >> 3) << 3);
        const MAX_FLOAT: f64 = f64::from_bits((f64::MAX.to_bits() >> 3) << 3);

        // Test types
        assert_eq!(Value::int(0).get_type(), Type::Integer);
        assert_eq!(Value::float(0.0).get_type(), Type::Float);
        assert_eq!(Value::bool(false).get_type(), Type::Bool);

        // Test mid values
        assert_eq!(Value::int(0).as_int(), 0);
        assert_eq!(Value::float(0.0).as_float(), 0.0);

        // Test min values
        assert_eq!(Value::int(i64::MIN << 3).as_int(), i64::MIN << 3);
        assert_eq!(Value::float(MIN_FLOAT).as_float(), MIN_FLOAT);

        // Test max values
        assert_eq!(Value::int(i64::MAX >> 3).as_int(), i64::MAX >> 3);
        assert_eq!(Value::float(MAX_FLOAT).as_float(), MAX_FLOAT);

        assert_eq!(Value::bool(false).as_int(), 0);
        assert_eq!(Value::bool(true).as_int(), 1);

        assert_eq!(Value::int(32).as_int(), 32);
        assert_eq!(Value::default().as_int(), 0);
        assert!(1.2 - Value::float(1.2).as_float() < 0.0000_0000_0000_1);

        assert_eq!(Value::default().raw(), 0);
        assert_eq!(Value::int(13).raw(), 13);
        // Special case since, the precision is a bit lost when tagging
        // it will be off by the last 3 bits which are used for mask.
        // Generally there should not be any issues when doing regular
        // math apart from having precision loss for very-very large numbers
        assert_eq!(
            Value::float(1.2).raw(),
            (((1.2_f64).to_bits() >> 3) << 3) | Type::Float as u64
        );
    }
}
