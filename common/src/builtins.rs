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

/// Built-in `IoError` enum name (virtual `io` module).
pub const BUILTIN_IO_ERROR_ENUM: &str = "IoError";

/// `IoError` variants in tag order.
pub const BUILTIN_IO_ERROR_VARIANTS: &[&str] = &[
    "WouldBlock",
    "NotFound",
    "PermissionDenied",
    "AlreadyClosed",
    "InvalidInput",
    "Other",
];

/// Built-in `ErrorKind` enum name (virtual `ffi` module).
pub const BUILTIN_FFI_ERROR_KIND_ENUM: &str = "ErrorKind";

/// `ErrorKind` variants in tag order (userland FFI failures).
pub const BUILTIN_FFI_ERROR_KIND_VARIANTS: &[&str] = &[
    "LibraryNotFound",
    "SymbolNotFound",
    "ArityMismatch",
    "Libffi",
    "InvalidSignature",
    "InvalidHandle",
    "Unsupported",
    "Other",
];

/// Built-in `Error` enum name (virtual `ffi` module).
///
/// Single record variant `Error { kind: ErrorKind, message: string }` so
/// callers can check `e.kind` and read `e.message` without string matching.
pub const BUILTIN_FFI_ERROR_ENUM: &str = "Error";

/// Sole variant of [`BUILTIN_FFI_ERROR_ENUM`].
pub const BUILTIN_FFI_ERROR_VARIANT: &str = "Error";

/// True when `name` is a reserved built-in enum (`Option`, `Result`, `IoError`,
/// `Error` / `ErrorKind`, or `FFIType`).
pub fn is_builtin_enum(name: &str) -> bool {
    is_builtin_option_enum(name)
        || is_builtin_result_enum(name)
        || is_builtin_io_error_enum(name)
        || is_builtin_ffi_error_enum(name)
        || is_builtin_ffi_error_kind_enum(name)
        || is_builtin_ffi_enum(name)
}

pub fn is_builtin_io_error_enum(name: &str) -> bool {
    name == BUILTIN_IO_ERROR_ENUM
}

pub fn is_builtin_ffi_error_enum(name: &str) -> bool {
    name == BUILTIN_FFI_ERROR_ENUM
}

pub fn is_builtin_ffi_error_kind_enum(name: &str) -> bool {
    name == BUILTIN_FFI_ERROR_KIND_ENUM
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
