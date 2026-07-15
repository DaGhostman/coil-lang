//! libffi call preparation and invocation.

use std::ffi::{CStr, CString, c_char, c_void};

use common::Value;
use libffi::middle::{Arg, Cif, CodePtr, Type};

use crate::memory::{CStructLayout, FfiType, Heap, Member, ObjString, Object};

use super::signature::{FfiError, FfiSignature};

pub struct PreparedCall {
    pub cif: Cif,
    pub addr: CodePtr,
}

pub struct InvokeContext {
    heap: *mut Heap,
    struct_layouts: *const CStructLayout,
    struct_layouts_len: usize,
}

impl InvokeContext {
    pub fn new(heap: *mut Heap, struct_layouts: &[CStructLayout]) -> Self {
        Self {
            heap,
            struct_layouts: struct_layouts.as_ptr(),
            struct_layouts_len: struct_layouts.len(),
        }
    }

    fn heap(&mut self) -> &mut Heap {
        // SAFETY: VM is single-threaded; reentrant callbacks may borrow the heap
        // while libffi is active, so this cannot overlap with `&mut Heap` borrows
        // held across the native call.
        unsafe { &mut *self.heap }
    }

    fn layouts(&self) -> &[CStructLayout] {
        unsafe { std::slice::from_raw_parts(self.struct_layouts, self.struct_layouts_len) }
    }
}

fn ffi_type_to_libffi(ty: FfiType, layouts: &[CStructLayout]) -> Result<Type, FfiError> {
    match ty {
        FfiType::Int => Ok(Type::i64()),
        FfiType::Float => Ok(Type::f64()),
        FfiType::String | FfiType::Ptr | FfiType::Callback(_) => Ok(Type::pointer()),
        FfiType::Void => Ok(Type::void()),
        FfiType::Bool => Ok(Type::u8()),
        FfiType::Int8 => Ok(Type::i8()),
        FfiType::Int16 => Ok(Type::i16()),
        FfiType::Int32 => Ok(Type::i32()),
        FfiType::UInt8 => Ok(Type::u8()),
        FfiType::UInt16 => Ok(Type::u16()),
        FfiType::UInt32 => Ok(Type::u32()),
        FfiType::UInt64 => Ok(Type::u64()),
        FfiType::Struct(id) => {
            let layout = layouts
                .get(id as usize)
                .ok_or_else(|| FfiError::Unsupported(format!("unknown struct layout id {id}")))?;
            let fields: Result<Vec<Type>, FfiError> = layout
                .fields
                .iter()
                .map(|(_, fty)| ffi_type_to_libffi(*fty, layouts))
                .collect();
            Ok(Type::structure(fields?))
        }
    }
}

pub fn prepare_cif(sig: &FfiSignature, layouts: &[CStructLayout]) -> Result<PreparedCall, FfiError> {
    let arg_types: Result<Vec<Type>, FfiError> = sig
        .args
        .iter()
        .copied()
        .map(|t| ffi_type_to_libffi(t, layouts))
        .collect();
    let ret_type = ffi_type_to_libffi(sig.ret, layouts)?;
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
    layouts: &[CStructLayout],
) -> Result<PreparedCall, FfiError> {
    let mut prepared = prepare_cif(sig, layouts)?;
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

fn member_to_value(member: &Member) -> Value {
    match member {
        Member::Value(v) => Value::from(*v),
        Member::Object(o) => Value::from(o.addr()),
    }
}

fn instance_field(heap: &mut Heap, addr: u64, fname: &str) -> Result<Value, FfiError> {
    let obj = heap
        .find_object_by_addr(addr)
        .ok_or_else(|| FfiError::Unsupported("struct value not found on heap".into()))?;
    match obj {
        Object::Instance(gc) => {
            let key = heap.intern(fname.to_string());
            gc.as_ref()
                .get(key)
                .map(|member| member_to_value(&member))
                .ok_or_else(|| FfiError::Unsupported(format!("missing field `{fname}`")))
        }
        _ => Err(FfiError::Unsupported(
            "struct argument must be a record/dict instance".into(),
        )),
    }
}

fn append_field_bytes(
    out: &mut Vec<u8>,
    val: &Value,
    fty: FfiType,
    heap: &mut Heap,
    layouts: &[CStructLayout],
) -> Result<(), FfiError> {
    match fty {
        FfiType::Int => out.extend_from_slice(&val.as_int().to_ne_bytes()),
        FfiType::Int8 => out.push(val.as_int() as i8 as u8),
        FfiType::Int16 => out.extend_from_slice(&(val.as_int() as i16).to_ne_bytes()),
        FfiType::Int32 => out.extend_from_slice(&(val.as_int() as i32).to_ne_bytes()),
        FfiType::UInt8 => out.push(val.as_int() as u8),
        FfiType::UInt16 => out.extend_from_slice(&(val.as_int() as u16).to_ne_bytes()),
        FfiType::UInt32 => out.extend_from_slice(&(val.as_int() as u32).to_ne_bytes()),
        FfiType::UInt64 => out.extend_from_slice(&(val.as_int() as u64).to_ne_bytes()),
        FfiType::Float => out.extend_from_slice(&val.as_float().to_ne_bytes()),
        FfiType::Bool => out.push(if val.as_bool() { 1 } else { 0 }),
        FfiType::Struct(id) => {
            let sub = layouts
                .get(id as usize)
                .ok_or_else(|| FfiError::Unsupported(format!("unknown nested struct id {id}")))?;
            pack_struct(heap, val, sub, layouts, out)?;
        }
        _ => {
            return Err(FfiError::Unsupported(format!(
                "field type `{fty:?}` not supported in struct pack"
            )));
        }
    }
    Ok(())
}

fn pack_struct(
    heap: &mut Heap,
    value: &Value,
    layout: &CStructLayout,
    layouts: &[CStructLayout],
    out: &mut Vec<u8>,
) -> Result<(), FfiError> {
    out.clear();
    let addr = value.raw() as u64;
    for (fname, fty) in &layout.fields {
        let val = instance_field(heap, addr, fname)?;
        append_field_bytes(out, &val, *fty, heap, layouts)?;
    }
    Ok(())
}

fn array_buffer_from_value(
    heap: &Heap,
    value: &Value,
    bufs: &mut Vec<Vec<i64>>,
) -> Result<(*mut c_void, Option<u64>), FfiError> {
    let addr = value.raw() as u64;
    if let Some(obj) = heap.find_object_by_addr(addr) {
        let elements = match obj {
            Object::Array(gc) => gc.as_ref().elements.clone(),
            Object::Tuple(gc) => gc.as_ref().elements.clone(),
            _ => Vec::new(),
        };
        if !elements.is_empty() {
            let mut buf: Vec<i64> = elements.iter().map(|v| v.as_int()).collect();
            let ptr = buf.as_mut_ptr() as *mut c_void;
            bufs.push(buf);
            return Ok((ptr, Some(addr)));
        }
    }
    Ok((value.raw() as *mut c_void, None))
}

fn copy_array_buffers_back(heap: &mut Heap, targets: &[(u64, usize)], bufs: &[Vec<i64>]) {
    for &(addr, buf_idx) in targets {
        let Some(buf) = bufs.get(buf_idx) else {
            continue;
        };
        heap.update_array_elements(addr, buf);
    }
}

pub fn invoke_via_libffi(
    prepared: &PreparedCall,
    sig: &FfiSignature,
    args: &[Value],
    ctx: &mut InvokeContext,
    _callback_closures: &mut Vec<*mut c_void>,
) -> Result<Option<Value>, FfiError> {
    if args.len() != sig.arity() {
        return Err(FfiError::ArityMismatch {
            expected: sig.arity(),
            got: args.len(),
        });
    }

    let mut i64_storage: Vec<i64> = Vec::new();
    let mut i8_storage: Vec<i8> = Vec::new();
    let mut i16_storage: Vec<i16> = Vec::new();
    let mut i32_storage: Vec<i32> = Vec::new();
    let mut u8_storage: Vec<u8> = Vec::new();
    let mut u16_storage: Vec<u16> = Vec::new();
    let mut u32_storage: Vec<u32> = Vec::new();
    let mut u64_storage: Vec<u64> = Vec::new();
    let mut f64_storage: Vec<f64> = Vec::new();
    let mut str_storage: Vec<CString> = Vec::new();
    let mut ptr_storage: Vec<*mut c_void> = Vec::new();
    let mut array_buffers: Vec<Vec<i64>> = Vec::new();
    let mut array_copy_back: Vec<(u64, usize)> = Vec::new();
    let mut struct_bufs: Vec<Vec<u8>> = Vec::new();
    let mut ffi_args: Vec<Arg> = Vec::with_capacity(sig.arity());

    for (i, (ty, value)) in sig.args.iter().zip(args.iter()).enumerate() {
        match ty {
            FfiType::Int => {
                i64_storage.push(value.as_int());
                ffi_args.push(Arg::new(i64_storage.last().unwrap()));
            }
            FfiType::Int8 => {
                i8_storage.push(value.as_int() as i8);
                ffi_args.push(Arg::new(i8_storage.last().unwrap()));
            }
            FfiType::Int16 => {
                i16_storage.push(value.as_int() as i16);
                ffi_args.push(Arg::new(i16_storage.last().unwrap()));
            }
            FfiType::Int32 => {
                i32_storage.push(value.as_int() as i32);
                ffi_args.push(Arg::new(i32_storage.last().unwrap()));
            }
            FfiType::UInt8 => {
                u8_storage.push(value.as_int() as u8);
                ffi_args.push(Arg::new(u8_storage.last().unwrap()));
            }
            FfiType::UInt16 => {
                u16_storage.push(value.as_int() as u16);
                ffi_args.push(Arg::new(u16_storage.last().unwrap()));
            }
            FfiType::UInt32 => {
                u32_storage.push(value.as_int() as u32);
                ffi_args.push(Arg::new(u32_storage.last().unwrap()));
            }
            FfiType::UInt64 => {
                u64_storage.push(value.as_int() as u64);
                ffi_args.push(Arg::new(u64_storage.last().unwrap()));
            }
            FfiType::Float => {
                f64_storage.push(value.as_float());
                ffi_args.push(Arg::new(f64_storage.last().unwrap()));
            }
            FfiType::Bool => {
                u8_storage.push(if value.as_bool() { 1 } else { 0 });
                ffi_args.push(Arg::new(u8_storage.last().unwrap()));
            }
            FfiType::String => {
                let heap = ctx.heap();
                let ptr = read_c_string_ptr(heap, value);
                if ptr.is_null() {
                    str_storage.push(CString::new("").unwrap());
                } else {
                    let s = unsafe { CStr::from_ptr(ptr) };
                    str_storage.push(CString::new(s.to_bytes()).unwrap_or_default());
                }
                let c_ptr = str_storage.last().unwrap().as_ptr();
                ffi_args.push(Arg::new(&c_ptr));
            }
            FfiType::Ptr => {
                let (ptr, heap_addr) =
                    array_buffer_from_value(ctx.heap(), value, &mut array_buffers)?;
                if let Some(addr) = heap_addr {
                    array_copy_back.push((addr, array_buffers.len() - 1));
                }
                ptr_storage.push(ptr);
                ffi_args.push(Arg::new(ptr_storage.last().unwrap()));
            }
            FfiType::Callback(_) => {
                ptr_storage.push(value.raw() as *mut c_void);
                ffi_args.push(Arg::new(ptr_storage.last().unwrap()));
            }
            FfiType::Struct(id) => {
                let layouts: Vec<CStructLayout> = ctx.layouts().to_vec();
                let layout = layouts
                    .get(*id as usize)
                    .ok_or_else(|| FfiError::Unsupported(format!("unknown struct layout id {id}")))?
                    .clone();
                let mut buf = Vec::new();
                pack_struct(ctx.heap(), value, &layout, &layouts, &mut buf)?;
                struct_bufs.push(buf);
                let ptr = struct_bufs.last().unwrap().as_ptr() as *mut c_void;
                ffi_args.push(Arg::new(&ptr));
            }
            FfiType::Void => return Err(FfiError::VoidArgument { index: i }),
        }
    }

    match sig.ret {
        FfiType::Void => {
            unsafe {
                prepared.cif.call::<()>(prepared.addr, &ffi_args);
            }
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(None)
        }
        FfiType::Int | FfiType::Int32 | FfiType::Int16 | FfiType::Int8 => {
            let ret = unsafe { prepared.cif.call::<i64>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(Some(Value::from(ret)))
        }
        FfiType::UInt8 | FfiType::UInt16 | FfiType::UInt32 | FfiType::UInt64 => {
            let ret = unsafe { prepared.cif.call::<u64>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(Some(Value::from(ret as i64)))
        }
        FfiType::Float => {
            let ret = unsafe { prepared.cif.call::<f64>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(Some(Value::from(ret)))
        }
        FfiType::Bool => {
            let ret = unsafe { prepared.cif.call::<u8>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(Some(Value::from(ret != 0)))
        }
        FfiType::String => {
            let ret = unsafe { prepared.cif.call::<*mut c_char>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            if ret.is_null() {
                Ok(Some(Value::from(0u64)))
            } else {
                let s = unsafe { CStr::from_ptr(ret) };
                let data = s.to_string_lossy();
                let (obj, _gc) = ctx
                    .heap()
                    .alloc(ObjString::from(data.as_ref()), Object::String);
                Ok(Some(Value::from(obj.addr())))
            }
        }
        FfiType::Ptr => {
            let ret = unsafe { prepared.cif.call::<*mut c_void>(prepared.addr, &ffi_args) };
            copy_array_buffers_back(ctx.heap(), &array_copy_back, &array_buffers);
            Ok(Some(Value::from(ret as u64)))
        }
        FfiType::Struct(_) | FfiType::Callback(_) => Err(FfiError::Unsupported(
            "struct/callback return types not yet supported".into(),
        )),
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
        assert!(prepare_cif(&sig, &[]).is_ok());
    }

    #[test]
    fn invoke_rust_fn_via_libffi() {
        let sig = FfiSignature::from_parts("add", vec![FfiType::Int, FfiType::Int], FfiType::Int)
            .unwrap();
        let mut prepared = prepare_cif(&sig, &[]).unwrap();
        prepared.addr = CodePtr::from_ptr(add_two as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(40_i64), Value::from(2_i64)];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures)
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
        let mut prepared = prepare_cif(&sig, &[]).unwrap();
        prepared.addr = CodePtr::from_ptr(touch as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(3_i64)];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures).unwrap();
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
        let mut prepared = prepare_cif(&sig, &[]).unwrap();
        prepared.addr = CodePtr::from_ptr(mul as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(2.5_f64), Value::from(4.0_f64)];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures)
            .unwrap()
            .unwrap();
        assert!((ret.as_float() - 10.0).abs() < f64::EPSILON);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn invoke_libc_strlen_via_libffi() {
        let lib = match crate::ffi::resolve_library("c", None, &[]) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("skipping: libc not reachable via dlopen");
                return;
            }
        };
        let sig = FfiSignature::from_parts("strlen", vec![FfiType::String], FfiType::Int).unwrap();
        let prepared = match prepare_cif_for_symbol(&sig, &lib, "strlen", &[]) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let mut heap = Heap::default();
        let (obj, _gc) = heap.alloc(ObjString::from("hello"), Object::String);
        let args = [Value::from(obj.addr())];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures)
            .unwrap()
            .unwrap();
        assert_eq!(ret.as_int(), 5);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn apply_cb_rust_callback() {
        extern "C" fn doubler(x: i64) -> i64 {
            x * 2
        }
        let lib_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/libsum.so");
        if !lib_path.exists() {
            eprintln!("skipping: libsum.so not built");
            return;
        }
        let lib = match crate::ffi::resolve_library(lib_path.to_str().unwrap(), None, &[]) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let sig = FfiSignature::from_parts(
            "apply_cb",
            vec![FfiType::Callback(0), FfiType::Int],
            FfiType::Int,
        )
        .unwrap();
        let prepared = prepare_cif_for_symbol(&sig, &lib, "apply_cb", &[]).unwrap();
        let mut heap = Heap::default();
        let args = [
            Value::from(doubler as *const () as u64),
            Value::from(21_i64),
        ];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures)
            .unwrap()
            .unwrap();
        assert_eq!(ret.as_int(), 42);
    }

    #[test]
    fn libffi_closure_int_to_int_trampoline_only() {
        use libffi::middle::{Cif, Type};
        use std::ffi::c_void;
        use std::sync::atomic::{AtomicI64, Ordering};
        static LAST: AtomicI64 = AtomicI64::new(0);
        unsafe extern "C" fn tramp(
            _cif: &libffi::low::ffi_cif,
            result: &mut i64,
            args: *const *const c_void,
            _userdata: &(),
        ) {
            let arg_ptr = *args;
            let arg = *(arg_ptr as *const i64);
            LAST.store(arg, Ordering::SeqCst);
            *result = arg * 2;
        }
        let cif = Cif::new(vec![Type::i64()], Type::i64());
        let ud = ();
        let closure = libffi::middle::Closure::new(cif, tramp, &ud);
        type Cb = unsafe extern "C" fn(i64) -> i64;
        let cb: Cb = unsafe { std::mem::transmute(*closure.code_ptr()) };
        assert_eq!(unsafe { cb(21) }, 42);
        assert_eq!(LAST.load(Ordering::SeqCst), 21);
    }

    #[test]
    fn invoke_fails_on_wrong_arity() {
        let sig = FfiSignature::from_parts("add", vec![FfiType::Int, FfiType::Int], FfiType::Int)
            .unwrap();
        let mut prepared = prepare_cif(&sig, &[]).unwrap();
        prepared.addr = CodePtr::from_ptr(add_two as *mut c_void);
        let mut heap = Heap::default();
        let args = [Value::from(1_i64)];
        let mut ctx = InvokeContext::new(&mut heap, &[]);
        let mut closures = Vec::new();
        let err = invoke_via_libffi(&prepared, &sig, &args, &mut ctx, &mut closures).unwrap_err();
        assert!(matches!(
            err,
            FfiError::ArityMismatch {
                expected: 2,
                got: 1
            }
        ));
    }
}
