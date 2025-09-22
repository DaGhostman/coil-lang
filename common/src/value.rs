type Storage = u64;

#[derive(Default, Copy, Clone)]
pub struct Value(*mut u8);

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::new(value as _)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::new(value as _)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::new(value.to_bits() as _)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::new(value as u8 as _)
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::new(value as _)
    }
}

impl<T> From<*mut T> for Value {
    fn from(value: *mut T) -> Self {
        Self::new(value as _)
    }
}

impl<'a> Value {
    const fn new(raw: Storage) -> Self {
        Self(raw as _)
    }

    /// Replace the current object value with a newly
    /// provided one and applies masking
    pub const fn replace(&mut self, value: Storage) {
        self.0 = value as _;
    }
}

impl<'a> Value {
    /// Casts the internal pointer value to i64
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::from(42).as_int(), 42);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_int(&self) -> i64 {
        self.0 as _
    }

    /// Casts the internal pointer value to bool
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::from(true).as_bool(), true);
    /// assert_eq!(Value::from(false).as_bool(), false);
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_bool(&self) -> bool {
        self.0 as u8 == 1
    }

    /// Casts the internal pointer value to float
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(1.2, Value::from(1.2).as_float());
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn as_float(&self) -> f64 {
        f64::from_bits(self.0 as _)
    }

    pub fn as_ptr<T>(&self) -> *mut T {
        self.raw() as _
        // NonNull::without_provenance(
        //     NonZero::new(self.raw() as _).expect("Invalid pointer address"),
        // )
    }

    /// Casts the internal pointer value to float
    ///
    /// ```
    /// use common::Value;
    ///
    /// assert_eq!(Value::from(42).raw(), 42);
    /// assert_eq!(Value::from(true).raw(), 1);
    /// assert_eq!(1.2_f64.to_bits() as usize , Value::from(1.2).raw());
    /// ```
    ///
    /// You would need to verify the type externally
    pub fn raw(&self) -> usize {
        self.0.addr() 
        // (self.0 as usize >> 3) as _
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0 as Storage,)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0 as Storage,)
    }
}

#[cfg(test)]
mod tests {
    use crate::Value;

    const MIN_FLOAT: f64 = f64::MIN;
    const MAX_FLOAT: f64 = f64::MAX;

    const MIN_INT: i64 = i64::MIN;
    const MAX_INT: i64 = i64::MAX;

    #[test]
    fn ptr_tagging() {
        // Test mid values
        assert_eq!(Value::from(0).as_int(), 0);
        assert_eq!(Value::from(0.0).as_float(), 0.0);

        // Test min values
        assert_eq!(Value::from(MIN_INT).as_int(), MIN_INT);
        assert_eq!(Value::from(MIN_FLOAT).as_float(), MIN_FLOAT);

        // Test max values
        assert_eq!(Value::from(MAX_INT).as_int(), MAX_INT);
        assert_eq!(Value::from(MAX_FLOAT).as_float(), MAX_FLOAT);

        assert_eq!(Value::from(false).as_int(), 0);
        assert_eq!(Value::from(true).as_int(), 1);

        assert_eq!(Value::from(32).as_int(), 32);
        assert_eq!(Value::default().as_int(), 0);
        assert_eq!(Value::from(1.2).as_float(), 1.2);

        assert_eq!(Value::default().raw(), 0);
        assert_eq!(Value::from(13).raw(), 13);
        assert_eq!(Value::from(1.2).raw(), (1.2_f64).to_bits() as usize);
    }
}
