mod array_vec;

#[macro_export]
macro_rules! promise {
    ($cond: expr) => {
        #[cfg(debug_assertions)]
        {
            debug_assert!($cond);
        }
        #[cfg(not(debug_assertions))]
        { 
            unsafe {
                std::hint::assert_unchecked($cond)
            }
        }
    };
}

#[inline]
#[cold]
fn cold() {}

#[inline]
pub fn likely(b: bool) -> bool {
    if !b { cold() }
    b
}

#[inline]
pub fn unlikely(b: bool) -> bool {
    if b { cold() }
    b
}

pub use array_vec::*;

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
