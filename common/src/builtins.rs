//! Compiler-provided built-in enums (`Option`, `Result`, `FFIType`).

pub use crate::ffi::{
    BUILTIN_FFI_TYPE_ENUM, BUILTIN_FFI_TYPE_VARIANTS, is_builtin_ffi_enum, is_builtin_ffi_variant,
};

/// Built-in `Option` enum name.
pub const BUILTIN_OPTION_ENUM: &str = "Option";

/// `Option` variants in tag order: `None` = 0, `Some` = 1.
pub const BUILTIN_OPTION_VARIANTS: &[&str] = &["None", "Some"];

/// Built-in `Result` enum name.
pub const BUILTIN_RESULT_ENUM: &str = "Result";

/// `Result` variants in tag order: `Ok` = 0, `Err` = 1.
pub const BUILTIN_RESULT_VARIANTS: &[&str] = &["Ok", "Err"];

/// True when `name` is a reserved built-in enum (`Option`, `Result`, or `FFIType`).
pub fn is_builtin_enum(name: &str) -> bool {
    is_builtin_option_enum(name) || is_builtin_result_enum(name) || is_builtin_ffi_enum(name)
}

pub fn is_builtin_option_enum(name: &str) -> bool {
    name == BUILTIN_OPTION_ENUM
}

pub fn is_builtin_result_enum(name: &str) -> bool {
    name == BUILTIN_RESULT_ENUM
}

/// True when `name` is a polymorphic built-in sum (`Option` or `Result`).
pub fn is_poly_builtin_enum(name: &str) -> bool {
    is_builtin_option_enum(name) || is_builtin_result_enum(name)
}
