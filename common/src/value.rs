use std::{num::NonZero, ptr::NonNull};

use crate::promise;

type Storage = u64;
const MASK: Storage = 0b111;

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
        dbg!(raw, raw as u64, ((raw as u64) << 3) >> 3);
        promise!((((raw as u64) << 3) >> 3) as *mut u8 == raw);

        Self((((raw as u64) << 3) | t as Storage) as _)
    }

    /// Retrieve the type of the value
    pub fn get_type(&self) -> Type {
        unsafe { std::mem::transmute((self.0 as Storage & MASK) as u8) }
    }
}

impl<'a> Value {
    /// Create a new integer value
    pub fn int(value: i64) -> Self {
        // promise!(((value << 3) >> 3) as i64 == value);

        Self::with_type(value as _, Type::Integer)
    }

    /// Create a new float value
    pub fn float(value: f32) -> Self {
        let bits = value.to_bits();
        // promise!((((bits as Storage) << 3) >> 3) as u32 == bits);

        Self::with_type(bits as _, Type::Float)
    }

    /// Create a new boolean value
    pub fn bool(value: bool) -> Self {
        // promise!((((value as u8) << 3) >> 3) == value as u8);

        Self::with_type(value as u8 as _, Type::Bool)
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
        let ptr = value.as_ptr();

        // promise!((((ptr as u64) << 3) >> 3) == ptr as u64);

        Self::with_type(ptr as _, Type::String)
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
        let ptr = value.as_ptr() as Storage;

        // promise!((ptr << 3) >> 3 == ptr);

        Self::with_type(ptr as _, Type::Object)
    }

    /// Replace the current object value with a newly
    /// provided one and applies masking
    pub fn replace(&mut self, value: Storage) {
        promise!((value << 3) >> 3 == value);

        self.0 = ((value << 3) | (self.0 as Storage & MASK)) as _
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
    pub fn as_int(&self) -> i64 {
        self.raw() as _
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
    pub fn as_bool(&self) -> bool {
        self.raw() == 1
    }

    /// Casts the internal pointer value to float
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert!(1.2 - Value::float(1.2).as_float() < 0.0000_0000_0000_1);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_float(&self) -> f32 {
        f32::from_bits(self.raw() as _)
    }

    pub fn as_ptr<T>(&self) -> NonNull<T> {
        NonNull::without_provenance(
            NonZero::new(self.raw() as usize).expect("Invalid pointer address"),
        )
    }

    /// Casts the internal pointer value to float
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::int(42).raw(), 42);
    /// assert_eq!(Value::bool(true).raw(), 1);
    /// assert_eq!(1.2_f32.to_bits() , Value::float(1.2).raw() as u32);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn raw(&self) -> Storage {
        (self.0 as usize >> 3) as _
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.get_type() {
                Type::Null => "null".to_string(),
                Type::Integer => format!("int({})", self.as_int()),
                Type::Float => format!("float({:.?})", self.as_float()),
                Type::Bool => format!("bool({})", self.as_bool()),
                Type::Object | Type::String => format!("0x{:016x}", self.raw()),
                _ => unreachable!("Unknown value type"),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{Type, Value};

    const MIN_FLOAT: f32 = f32::MIN;
    const MAX_FLOAT: f32 = f32::MAX;

    const MIN_INT: i64 = (((i64::MIN as u64) << 3) >> 3) as i64;
    const MAX_INT: i64 = (((i64::MAX as u64) << 3) >> 3) as i64;

    #[test]
    fn ptr_tagging() {
        // Test types
        assert_eq!(Value::int(0).get_type(), Type::Integer);
        assert_eq!(Value::float(0.0).get_type(), Type::Float);
        assert_eq!(Value::bool(false).get_type(), Type::Bool);

        // Test mid values
        assert_eq!(Value::int(0).as_int(), 0);
        assert_eq!(Value::float(0.0).as_float(), 0.0);

        // Test min values
        assert_eq!(Value::int(MIN_INT).as_int(), MIN_INT);
        assert_eq!(Value::float(MIN_FLOAT).as_float(), MIN_FLOAT);

        // Test max values
        assert_eq!(Value::int(MAX_INT).as_int(), MAX_INT);
        assert_eq!(Value::float(MAX_FLOAT).as_float(), MAX_FLOAT);

        assert_eq!(Value::bool(false).as_int(), 0);
        assert_eq!(Value::bool(true).as_int(), 1);

        assert_eq!(Value::int(32).as_int(), 32);
        assert_eq!(Value::default().as_int(), 0);
        assert_eq!(Value::float(1.2).as_float(), 1.2);

        assert_eq!(Value::default().raw(), 0);
        assert_eq!(Value::int(13).raw(), 13);
        assert_eq!(Value::float(1.2).raw(), (1.2_f32).to_bits() as u64);
    }
}
