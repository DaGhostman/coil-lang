//! FFI (Foreign Function Interface) machinery.
//!
//! All FFI calls go through explicit [`FfiSignature`] values and libffi
//! dynamic dispatch. There is no signature guessing at runtime.

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

/// Register `sig` on a loaded library object, resolving the symbol
/// and preparing a libffi call interface at declare time.
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

/// Load a dynamic library by short name (e.g. `"c"`, `"m"`).
pub fn load_library(name: &str) -> Result<Arc<Library>, libloading::Error> {
    let lib = unsafe { Library::new(name) }?;
    Ok(Arc::new(lib))
}
