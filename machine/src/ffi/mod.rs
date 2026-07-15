//! FFI: explicit signatures and libffi dispatch (no runtime guessing).
//!
//! ## C ABI mapping (`FfiType` → libffi)
//! - `Int` → `i64`
//! - `Float` → `f64`
//! - `String` → `const char *` (heap `ObjString` address); C returns are copied
//! - `Void` → void (invoke pushes nothing)
//!
//! ## VM opcode stack (bottom → top)
//! - **`FfiLoad`**: `path_string` → `lib_handle`
//! - **`DeclareFFI`**: `lib`, `name`, `args_tuple` (type tags), `ret_tag` → `fn_id` (or `-1`)
//! - **`FfiInvoke`**: `lib`, `fn_id`, `args_tuple` → return value (void: no push)
//! - **`HostInvoke`**: `fn_id`, `args_tuple` → return value

mod call;
mod registry;
mod signature;

pub use call::{
    PreparedCall, invoke_via_libffi, prepare_cif, prepare_cif_for_symbol, resolve_symbol,
};
pub use libloading::Library;
pub use registry::{HostClosureFn, NativeFn, Natives};
pub use signature::{FfiError, FfiSignature, FfiSignatureBuilder};

use std::sync::Arc;

pub fn register_on_library(
    obj_lib: &mut crate::memory::ObjLibrary,
    sig: FfiSignature,
) -> Result<usize, FfiError> {
    let prepared = prepare_cif_for_symbol(&sig, &obj_lib.library, &sig.name)?;
    let id = obj_lib.signatures.len();
    let name = sig.name.clone();
    obj_lib.signatures.push(crate::memory::RegisteredFunction {
        sig: crate::memory::FunctionSig::from_ffi_signature(&sig),
        prepared,
    });
    obj_lib.by_name.insert(name, id);
    Ok(id)
}

pub fn load_library(name: &str) -> Result<Arc<Library>, libloading::Error> {
    let lib = unsafe { Library::new(name) }?;
    Ok(Arc::new(lib))
}
