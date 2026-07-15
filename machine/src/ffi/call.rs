//! libffi call preparation and invocation.

use std::ffi::{CStr, CString, c_char, c_void};

use common::Value;
use libffi::middle::{Arg, Cif, CodePtr, Type};

use crate::memory::{FfiType, Heap, ObjString, Object};

use super::signature::{FfiError, FfiSignature};

pub struct PreparedCall {
    pub cif: Cif,
    pub addr: CodePtr,
}

// ABI mapping: Int→i64, Float→f64, String→pointer, Void→void.
fn ffi_type_to_libffi(ty: FfiType) -> Result<Type, FfiError> {
    match ty {
        FfiType::Int => Ok(Type::i64()),
        FfiType::Float => Ok(Type::f64()),
        FfiType::String => Ok(Type::pointer()),
        FfiType::Void => Ok(Type::void()),
    }
}

pub fn prepare_cif(sig: &FfiSignature) -> Result<PreparedCall, FfiError> {
    let arg_types: Result<Vec<Type>, FfiError> =
        sig.args.iter().copied().map(ffi_type_to_libffi).collect();
    let ret_type = ffi_type_to_libffi(sig.ret)?;
    let cif = Cif::new(arg_types?, ret_type);
    Ok(PreparedCall {
        cif,
        addr: CodePtr::from_ptr(std::ptr::null_mut()),
    })
}

pub fn prepare_cif_for_symbol(
    sig: &FfiSignature,
    library: &libloading::Library,
    symbol: &str,
) -> Result<PreparedCall, FfiError> {
    let mut prepared = prepare_cif(sig)?;
    prepared.addr = resolve_symbol(library, symbol)?;
    Ok(prepared)
}

pub fn resolve_symbol(library: &libloading::Library, symbol: &str) -> Result<CodePtr, FfiError> {
    type FnPtr = unsafe extern "C" fn();
    let sym_bytes: &[u8] = symbol.as_bytes();
    let sym: libloading::Symbol<FnPtr> = unsafe {
        library
            .get(sym_bytes)
            .map_err(|_| FfiError::SymbolNotFound {
                name: symbol.to_string(),
            })?
    };
    let ptr: *mut c_void = unsafe { std::mem::transmute(sym.into_raw()) };
    Ok(CodePtr::from_ptr(ptr))
}

fn read_c_string_ptr(heap: &Heap, value: &Value) -> *const c_char {
    let raw = value.raw() as u64;
    if raw == 0 {
        return std::ptr::null();
    }
    heap.cstr_from_addr(raw).unwrap_or(std::ptr::null())
}

pub fn invoke_via_libffi(
    prepared: &PreparedCall,
    sig: &FfiSignature,
    args: &[Value],
    heap: &mut Heap,
) -> Result<Option<Value>, FfiError> {
    if args.len() != sig.arity() {
        return Err(FfiError::ArityMismatch {
            expected: sig.arity(),
            got: args.len(),
        });
    }

    let mut i64_storage: Vec<i64> = Vec::new();
    let mut f64_storage: Vec<f64> = Vec::new();
    let mut str_storage: Vec<CString> = Vec::new();
    let mut ffi_args: Vec<Arg> = Vec::with_capacity(sig.arity());

    for (ty, value) in sig.args.iter().zip(args.iter()) {
        match ty {
            FfiType::Int => {
                i64_storage.push(value.as_int());
                ffi_args.push(Arg::new(i64_storage.last().unwrap()));
            }
            FfiType::Float => {
                f64_storage.push(value.as_float());
                ffi_args.push(Arg::new(f64_storage.last().unwrap()));
            }
            FfiType::String => {
                let ptr = read_c_string_ptr(heap, value);
                if ptr.is_null() {
                    str_storage.push(CString::new("").unwrap());
                } else {
                    // SAFETY: `ptr` addresses a live `ObjString` on the heap.
                    let s = unsafe { CStr::from_ptr(ptr) };
                    str_storage.push(CString::new(s.to_bytes()).unwrap_or_default());
                }
                let c_ptr = str_storage.last().unwrap().as_ptr();
                ffi_args.push(Arg::new(&c_ptr));
            }
            FfiType::Void => return Err(FfiError::VoidArgument { index: 0 }),
        }
    }

    match sig.ret {
        FfiType::Void => {
            unsafe {
                prepared.cif.call::<()>(prepared.addr, &ffi_args);
            }
            Ok(None)
        }
        FfiType::Int => {
            let ret = unsafe { prepared.cif.call::<i64>(prepared.addr, &ffi_args) };
            Ok(Some(Value::from(ret)))
        }
        FfiType::Float => {
            let ret = unsafe { prepared.cif.call::<f64>(prepared.addr, &ffi_args) };
            Ok(Some(Value::from(ret)))
        }
        FfiType::String => {
            let ret = unsafe { prepared.cif.call::<*mut c_char>(prepared.addr, &ffi_args) };
            if ret.is_null() {
                Ok(Some(Value::from(0u64)))
            } else {
                // SAFETY: `ret` is a valid C string for this read; copied into `ObjString`.
                let s = unsafe { CStr::from_ptr(ret) };
                let data = s.to_string_lossy();
                let (obj, _gc) = heap.alloc(ObjString::from(data.as_ref()), Object::String);
                Ok(Some(Value::from(obj.addr())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::load_library;

    extern "C" fn add_two(a: i64, b: i64) -> i64 {
        a + b
    }

    #[test]
    fn prepare_cif_accepts_int_int_to_int() {
        let sig = FfiSignature::from_parts("add", vec![FfiType::Int, FfiType::Int], FfiType::Int)
            .unwrap();
        assert!(prepare_cif(&sig).is_ok());
    }

    #[test]
    fn invoke_rust_fn_via_libffi() {
        let sig = FfiSignature::from_parts("add", vec![FfiType::Int, FfiType::Int], FfiType::Int)
            .unwrap();
        let mut prepared = prepare_cif(&sig).unwrap();
        prepared.addr = CodePtr::from_ptr(add_two as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(40_i64), Value::from(2_i64)];
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut heap)
            .unwrap()
            .unwrap();
        assert_eq!(ret.as_int(), 42);
    }

    #[test]
    fn invoke_void_return_pushes_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static HITS: AtomicUsize = AtomicUsize::new(0);
        extern "C" fn touch(v: i64) {
            HITS.fetch_add(v as usize, Ordering::SeqCst);
        }
        let sig = FfiSignature::from_parts("touch", vec![FfiType::Int], FfiType::Void).unwrap();
        let mut prepared = prepare_cif(&sig).unwrap();
        prepared.addr = CodePtr::from_ptr(touch as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(3_i64)];
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut heap).unwrap();
        assert!(ret.is_none());
        assert_eq!(HITS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn invoke_float_round_trip() {
        extern "C" fn mul(a: f64, b: f64) -> f64 {
            a * b
        }
        let sig =
            FfiSignature::from_parts("mul", vec![FfiType::Float, FfiType::Float], FfiType::Float)
                .unwrap();
        let mut prepared = prepare_cif(&sig).unwrap();
        prepared.addr = CodePtr::from_ptr(mul as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(2.5_f64), Value::from(4.0_f64)];
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut heap)
            .unwrap()
            .unwrap();
        assert!((ret.as_float() - 10.0).abs() < f64::EPSILON);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn invoke_libc_strlen_via_libffi() {
        let lib = match load_library("c") {
            Ok(l) => l,
            Err(_) => {
                eprintln!("skipping: libc not reachable via dlopen");
                return;
            }
        };
        let sig = FfiSignature::from_parts("strlen", vec![FfiType::String], FfiType::Int).unwrap();
        let prepared = match prepare_cif_for_symbol(&sig, &lib, "strlen") {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let mut heap = Heap::default();
        let (obj, _gc) = heap.alloc(ObjString::from("hello"), Object::String);
        let args = [Value::from(obj.addr())];
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut heap)
            .unwrap()
            .unwrap();
        assert_eq!(ret.as_int(), 5);
    }

    #[test]
    fn register_on_library_fails_on_missing_symbol() {
        let lib = match load_library("c") {
            Ok(l) => l,
            Err(_) => {
                eprintln!("skipping: libc not reachable via dlopen");
                return;
            }
        };
        let mut obj_lib = crate::memory::ObjLibrary {
            library: lib,
            signatures: Vec::new(),
            by_name: std::collections::HashMap::new(),
        };
        let sig = FfiSignature::from_parts(
            "nonexistent_symbol_xyz_12345",
            vec![FfiType::Int],
            FfiType::Int,
        )
        .unwrap();
        let err = crate::ffi::register_on_library(&mut obj_lib, sig).unwrap_err();
        assert!(matches!(err, FfiError::SymbolNotFound { .. }));
    }

    #[test]
    fn invoke_fails_on_wrong_arity() {
        let sig = FfiSignature::from_parts("add", vec![FfiType::Int, FfiType::Int], FfiType::Int)
            .unwrap();
        let mut prepared = prepare_cif(&sig).unwrap();
        prepared.addr = CodePtr::from_ptr(add_two as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(1_i64)];
        let err = invoke_via_libffi(&prepared, &sig, &args, &mut heap).unwrap_err();
        assert!(matches!(
            err,
            FfiError::ArityMismatch {
                expected: 2,
                got: 1
            }
        ));
    }
}
