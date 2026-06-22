mod archive;
mod array_vec;
mod interner;
mod opcode;
mod value;
mod message;
mod seekable_iter;

pub use archive::*;
pub use array_vec::*;
pub use interner::*;
pub use opcode::*;
pub use value::*;
pub use message::*;
pub use seekable_iter::*;

#[macro_export]
macro_rules! promise {
    ($cond: expr) => {
        #[cfg(debug_assertions)]
        {
            debug_assert!($cond);
        }
        #[cfg(not(debug_assertions))]
        {
            unsafe { std::hint::assert_unchecked($cond) }
        }
    };
    ($cond: expr, $msg: literal) => {
        #[cfg(debug_assertions)]
        {
            debug_assert!($cond, $msg);
        }
        #[cfg(not(debug_assertions))]
        {
            unsafe { std::hint::assert_unchecked($cond) }
        }
    };
}

#[inline(always)]
#[cold]
fn cold() {}

#[inline(always)]
pub fn likely(b: bool) -> bool {
    {
        if !b {
            cold()
        }

        b
    }
}

#[inline(always)]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold()
    }

    b
}

// #[repr(u8)]
// pub enum Types {
//     NUMBER = 0,
//     STRING = 1,
// }
//
// impl Into<u8> for Types {
//     fn into(self) -> u8 {
//         self as u8
//     }
// }
//
// #[repr(u8)]
// pub enum Registers {
//     RET = 254,
// }
//
// impl Into<u8> for Registers {
//     fn into(self) -> u8 {
//         self as u8
//     }
// }
//
// impl Into<usize> for Registers {
//     fn into(self) -> usize {
//         (self as u8) as usize
//     }
// }
