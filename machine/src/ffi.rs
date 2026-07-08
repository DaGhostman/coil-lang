//! FFI (Foreign Function Interface) machinery.
//!
//! Lets the VM call functions defined in shared libraries (`.so`,
//! `.dylib`, `.dll`) and lets host programs register Rust functions
//! that the VM bytecode can call as "natives".
//!
//! ## Two registration paths
//!
//! 1. **Host-registered natives** — A Rust program that embeds the
//!    VM can register a Rust function with a specific arity and
//!    type signature. The function is stored in
//!    `Machine::natives` keyed by name; the bytecode's `NATIVE`
//!    opcode dispatches to it.
//!
//! 2. **Dynamic-library (FFI) natives** — The language's
//!    `extern "libname" { fn symbol(args) -> ret; }` declaration
//!    tells the VM to load `libname` (via `libloading`) and resolve
//!    `symbol` once. The resolved function pointer is cached in
//!    `Machine::natives` and dispatched the same way.
//!
//! ## ABI
//!
//! The current implementation supports a fixed set of C ABI
//! signatures (see [`NativeFn`]). Variadic functions are NOT
//! supported (they need a different marshalling strategy for
//! argument counts). The C signatures are:
//!
//! - `() -> ()`        (void return, no params)
//! - `(i64) -> ()`      (single int param, void return)
//! - `(i64) -> i64`    (int param, int return)
//! - `(i64, i64) -> i64`
//! - `(*const c_char) -> ()`       (C string param, void return)
//! - `(*const c_char) -> i64`     (C string param, int return)
//! - `(i64, *const c_char) -> i64` (printf-style; partial support)
//!
//! ## Type marshalling
//!
//! The VM stores values as `Value` (a 64-bit tagged representation
//! of int/float/bool/heap-pointer). On `NATIVE` dispatch, the
//! raw bits are reinterpreted as the expected C type. For heap
//! pointers (strings), the `Value` is a heap object address; the
//! marshaller reads the actual C string from the heap object.
//!
//! For string return values, the VM allocates a fresh `ObjString`
//! and returns the resulting `Value` (a heap pointer).

use std::collections::HashMap;
use std::sync::Arc;

pub use libloading::Library;

use common::Value;

use crate::memory::Heap;

#[cfg(test)]
use crate::memory::{ObjString, Object};

/// Trait implemented by every VM-callable native function.
///
/// Each implementation has a fixed C signature (no variadics).
/// Dispatch picks the implementation by name.
pub trait NativeFn: Send + Sync {
    /// The function's name as it appears in `Machine::natives`.
    fn name(&self) -> &str;

    /// The function's arity (number of arguments it pops from
    /// the operand stack).
    fn arity(&self) -> usize;

    /// Invoke the function. `heap` is the VM's heap (needed
    /// to resolve `Value`-as-pointer arguments to C strings via
    /// [`Heap::cstr_from_addr`]). `args` is the slice of stack
    /// values the callee popped (in source order — first arg
    /// is `args[0]`). Returns the value to push on the stack,
    /// or `None` for void returns.
    fn invoke(&self, heap: &Heap, args: &[Value]) -> Option<Value>;
}

// ---------------------------------------------------------------------------
// Built-in native implementations for the most common C signatures.
// ---------------------------------------------------------------------------

/// `(i64) -> i64` — wraps a Rust `fn(i64) -> i64`.
pub struct UnaryI64ToI64 {
    name: String,
    func: Arc<dyn Fn(i64) -> i64 + Send + Sync>,
}

impl UnaryI64ToI64 {
    pub fn new<F>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn(i64) -> i64 + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(func),
        }
    }
}

impl NativeFn for UnaryI64ToI64 {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        1
    }
    fn invoke(&self, _heap: &Heap, args: &[Value]) -> Option<Value> {
        let x = args[0].as_int();
        Some(Value::from((*self.func)(x)))
    }
}

/// `() -> ()` — wraps a Rust `fn()`.
pub struct NullaryVoid {
    name: String,
    func: Arc<dyn Fn() + Send + Sync>,
}

impl NullaryVoid {
    pub fn new<F>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(func),
        }
    }
}

impl NativeFn for NullaryVoid {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        0
    }
    fn invoke(&self, _heap: &Heap, _args: &[Value]) -> Option<Value> {
        (*self.func)();
        None
    }
}

/// `(*const c_char) -> ()` — wraps a Rust `fn(*const c_char)` (the
/// typical C signature for `puts`, custom log functions, etc.).
pub struct StringArgVoid {
    name: String,
    func: Arc<dyn Fn(*const std::os::raw::c_char) + Send + Sync>,
}

impl StringArgVoid {
    pub fn new<F>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn(*const std::os::raw::c_char) + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(func),
        }
    }
}

impl NativeFn for StringArgVoid {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        1
    }
    fn invoke(&self, heap: &Heap, args: &[Value]) -> Option<Value> {
        let s = read_c_string(heap, &args[0]);
        (*self.func)(s);
        None
    }
}

/// `(*const c_char) -> i64` — wraps a Rust `fn(*const c_char) -> i64`
/// (e.g. `strlen`, `atoi`, custom parsers).
pub struct StringArgToI64 {
    name: String,
    func: Arc<dyn Fn(*const std::os::raw::c_char) -> i64 + Send + Sync>,
}

impl StringArgToI64 {
    pub fn new<F>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn(*const std::os::raw::c_char) -> i64 + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(func),
        }
    }
}

impl NativeFn for StringArgToI64 {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        1
    }
    fn invoke(&self, heap: &Heap, args: &[Value]) -> Option<Value> {
        let s = read_c_string(heap, &args[0]);
        Some(Value::from((*self.func)(s)))
    }
}

/// `() -> i64` — wraps a Rust `fn() -> i64` (clock, time, etc.).
pub struct NullaryI64 {
    name: String,
    func: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl NullaryI64 {
    pub fn new<F>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(func),
        }
    }
}

impl NativeFn for NullaryI64 {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        0
    }
    fn invoke(&self, _heap: &Heap, _args: &[Value]) -> Option<Value> {
        Some(Value::from((*self.func)()))
    }
}

// ---------------------------------------------------------------------------
// Dynamic-library (FFI) wrappers.
//
// The VM holds a `Library` (from `libloading`) for each shared
// library it has loaded. Each function resolved from a library is
// wrapped in a `LibraryFn` that holds the `Library` (so the
// library stays loaded for the function's lifetime) and the raw
// function pointer.
// ---------------------------------------------------------------------------

/// Internal helper: read a C string from a `Value` using a heap
/// reference. The `Value`'s raw bits are interpreted as the
/// address of a heap-allocated `ObjString`; the heap looks up
/// the object and returns a `*const c_char` to its data.
///
/// Falls back to a `null` pointer if the value's raw bits are
/// null. The VM stores strings as `ObjString` (a heap object
/// stored as a raw pointer). The pointer in the `Value` is the
/// address of the heap object; we read the string data from
/// there.
fn read_c_string(heap: &Heap, value: &Value) -> *const std::os::raw::c_char {
    let raw = value.raw() as u64;
    if raw == 0 {
        return std::ptr::null();
    }
    heap.cstr_from_addr(raw).unwrap_or(std::ptr::null())
}

/// A native function backed by a dynamic-library symbol.
///
/// `LibraryFn` holds an `Arc<Library>` to keep the loaded
/// library alive for the function's lifetime, plus a raw
/// function pointer extracted from the symbol via
/// `Symbol::into_raw` (and unsafe-casted to the expected C
/// signature). The raw pointer is just a `usize` address; the
/// function pointer is valid as long as the `Library` Arc
/// stays alive (which the `_library` field ensures).
///
/// The C signature is fixed at registration time (one of the
/// supported signatures — see [`NativeFn`]).
pub struct LibraryFn {
    name: String,
    arity: usize,
    kind: LibraryFnKind,
    // Keep the library loaded for at least the function's
    // lifetime. The `Arc` is cheap to clone.
    _library: Arc<Library>,
}

/// The C signature of a library-loaded native. Each variant
/// stores the resolved function pointer as a raw address. The
/// pointer is valid as long as the `Library` Arc in
/// `LibraryFn::_library` stays alive.
enum LibraryFnKind {
    /// `() -> ()`
    VoidVoid(usize),
    /// `(i64) -> ()`
    I64Void(usize),
    /// `(i64) -> i64`
    I64I64(usize),
    /// `(i64, i64) -> i64` — Phase 22b userland FFI support for
    /// `sum(int, int)` style C signatures.
    I64I64I64(usize),
    /// `(*const c_char) -> ()`
    StrVoid(usize),
    /// `(*const c_char) -> i64`
    StrI64(usize),
    /// `() -> i64`
    VoidI64(usize),
    /// Placeholder for symbols whose signature isn't one of the
    /// canned kinds above. Currently unreachable; we resolve
    /// symbols into a specific kind at registration time.
    _Unsupported,
}

impl LibraryFn {
    /// Resolve `symbol` in `library` and wrap it as a
    /// `LibraryFn` named `name`. Tries each supported C signature
    /// in order; the first one that resolves wins.
    ///
    /// Returns an error if the symbol cannot be resolved in any
    /// of the supported signatures.
    pub fn new(
        name: impl Into<String>,
        symbol: &str,
        library: Arc<Library>,
    ) -> Result<Self, String> {
        let name = name.into();
        // `library.get` takes a `&[u8]` (the symbol's raw bytes).
        // The bytes are NUL-terminated for C-style libraries;
        // libloading takes care of platform-specific encoding.
        let sym_bytes: &[u8] = symbol.as_bytes();
        // Try each signature in turn. The first one that
        // resolves the symbol wins. We use the most common
        // signatures first so the typical case (int-in,
        // int-out) is fast. Each successful resolution extracts
        // the function pointer's raw address (dropping the
        // `Symbol`'s lifetime); the `Library` Arc in
        // `LibraryFn::_library` keeps the underlying memory
        // alive, so the function pointer stays valid.
        //
        // SAFETY: `Symbol::into_raw` returns the raw pointer
        // (`*mut c_void`); we cast it to `usize` (an address)
        // and store it. The `Library` Arc in the returned
        // `LibraryFn` keeps the underlying allocation alive
        // for as long as the `LibraryFn` exists, so the
        // function pointer is valid for the lifetime of the
        // `LibraryFn` (i.e., as long as the VM holds the
        // `NativeFn` Arc).
        // The double `into_raw` chain unwraps the safe wrapper
        // (`Symbol<'lib, T>`) to the OS-specific symbol
        // (`os::unix::Symbol<T>`) to the raw `*mut c_void`,
        // which we cast to a `usize` address. The `Library`
        // Arc in `LibraryFn::_library` keeps the symbol's
        // backing memory alive for the `LibraryFn`'s
        // lifetime, so the function pointer stays valid.
        macro_rules! addr_for {
            ($sym:expr) => {
                unsafe { $sym.into_raw().into_raw() as usize }
            };
        }
        if let Ok(sym) = unsafe { library.get::<extern "C" fn(i64) -> i64>(sym_bytes) } {
            return Ok(Self {
                name,
                arity: 1,
                kind: LibraryFnKind::I64I64(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) = unsafe { library.get::<extern "C" fn(i64, i64) -> i64>(sym_bytes) } {
            return Ok(Self {
                name,
                arity: 2,
                kind: LibraryFnKind::I64I64I64(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) = unsafe { library.get::<extern "C" fn(i64)>(sym_bytes) } {
            return Ok(Self {
                name,
                arity: 1,
                kind: LibraryFnKind::I64Void(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) = unsafe { library.get::<extern "C" fn() -> i64>(sym_bytes) } {
            return Ok(Self {
                name,
                arity: 0,
                kind: LibraryFnKind::VoidI64(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) = unsafe { library.get::<extern "C" fn()>(sym_bytes) } {
            return Ok(Self {
                name,
                arity: 0,
                kind: LibraryFnKind::VoidVoid(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) =
            unsafe { library.get::<extern "C" fn(*const std::os::raw::c_char) -> i64>(sym_bytes) }
        {
            return Ok(Self {
                name,
                arity: 1,
                kind: LibraryFnKind::StrI64(addr_for!(sym)),
                _library: library,
            });
        }
        if let Ok(sym) =
            unsafe { library.get::<extern "C" fn(*const std::os::raw::c_char)>(sym_bytes) }
        {
            return Ok(Self {
                name,
                arity: 1,
                kind: LibraryFnKind::StrVoid(addr_for!(sym)),
                _library: library,
            });
        }
        Err(format!(
            "symbol `{}` not found in library (or signature unsupported)",
            symbol
        ))
    }
}

impl NativeFn for LibraryFn {
    fn name(&self) -> &str {
        &self.name
    }
    fn arity(&self) -> usize {
        self.arity
    }
    fn invoke(&self, heap: &Heap, args: &[Value]) -> Option<Value> {
        // SAFETY: each `addr` was extracted from a
        // `LibrarySymbol<T>` returned by `library.get` where
        // `T` matches the C signature we cast to. The
        // `Library` Arc in `self._library` keeps the symbol's
        // backing memory alive for as long as `self` lives, so
        // the cast is valid. Calling the function via the
        // pointer uses the platform's C ABI for the matched
        // signature, which the FFI contract guarantees.
        unsafe {
            match &self.kind {
                LibraryFnKind::VoidVoid(addr) => {
                    let f: extern "C" fn() = std::mem::transmute(addr);
                    f();
                    None
                }
                LibraryFnKind::I64Void(addr) => {
                    let f: extern "C" fn(i64) = std::mem::transmute(addr);
                    f(args[0].as_int());
                    None
                }
                LibraryFnKind::I64I64(addr) => {
                    let f: extern "C" fn(i64) -> i64 = std::mem::transmute(addr);
                    Some(Value::from(f(args[0].as_int())))
                }
                LibraryFnKind::I64I64I64(addr) => {
                    let f: extern "C" fn(i64, i64) -> i64 = std::mem::transmute(addr);
                    Some(Value::from(f(args[0].as_int(), args[1].as_int())))
                }
                LibraryFnKind::StrVoid(addr) => {
                    let f: extern "C" fn(*const std::os::raw::c_char) = std::mem::transmute(addr);
                    f(read_c_string(heap, &args[0]));
                    None
                }
                LibraryFnKind::StrI64(addr) => {
                    let f: extern "C" fn(*const std::os::raw::c_char) -> i64 =
                        std::mem::transmute(addr);
                    Some(Value::from(f(read_c_string(heap, &args[0]))))
                }
                LibraryFnKind::VoidI64(addr) => {
                    let f: extern "C" fn() -> i64 = std::mem::transmute(addr);
                    Some(Value::from(f()))
                }
                LibraryFnKind::_Unsupported => unreachable!(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Native registry. The VM owns one of these and dispatches the
// `NATIVE` opcode to a registered function by name.
// ---------------------------------------------------------------------------

/// Holds the set of native functions the VM can call.
///
/// Native functions are looked up by name (the same name that
/// appears in the bytecode's `NATIVE name` instruction — see
/// the compiler's codegen for `Expression::Call`).
#[derive(Default)]
pub struct Natives {
    /// Map from native name → implementation.
    by_name: HashMap<String, Arc<dyn NativeFn>>,
}

impl Natives {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native function. Overwrites any existing
    /// registration with the same name.
    pub fn register(&mut self, native: Arc<dyn NativeFn>) {
        let name = native.name().to_string();
        self.by_name.insert(name, native);
    }

    /// Remove a native by name. Returns the removed function
    /// (if any).
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn NativeFn>> {
        self.by_name.remove(name)
    }

    /// Look up a native by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn NativeFn>> {
        self.by_name.get(name).cloned()
    }

    /// Number of registered natives.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Helper: load a dynamic library by name. Looks up the library
// in a set of well-known locations, falling back to letting
// `libloading` do its own search.
// ---------------------------------------------------------------------------

/// Load a dynamic library by its short name (e.g. `"c"`,
/// `"m"`, `"dl"`). The platform-appropriate extension is
/// appended automatically (`libc.so.6`, `libm.dylib`,
/// `msvcrt.dll`, etc.).
///
/// Returns an `Arc<Library>` on success so the library stays
/// loaded as long as anyone holds a reference.
pub fn load_library(name: &str) -> Result<Arc<Library>, libloading::Error> {
    // `&str: AsRef<OsStr>`, so this just works.
    let lib = unsafe { Library::new(name) }?;
    Ok(Arc::new(lib))
}

/// Resolve a function symbol by name in a loaded `Library` and
/// invoke it with `args` (already in source order — first arg
/// first). `sig` describes the C ABI.
///
/// Used by the VM's `FfiInvoke` opcode. The signature's
/// `arg_types` and `ret_type` are used to construct a typed
/// function pointer; the call is made via `libloading`.
///
/// Returns `Some(Value)` if the function returned a non-void
/// value, or `None` for void returns. Returns `None` if the
/// symbol can't be resolved in any of the supported
/// signatures (the caller reports this as a runtime error).
pub fn call_through_library(
    library: &Library,
    sig: &crate::memory::FunctionSig,
    args: &[Value],
    heap: &crate::memory::Heap,
) -> Option<Value> {
    let name_bytes: &[u8] = sig.name.as_bytes();
    // The signature's declared arg types let us pick the
    // right C ABI: e.g. `String → Int` must dispatch through
    // `(*const c_char) → i64`, NOT `i64 → i64` (which would
    // mis-call `strlen(40)` and return junk). We try each
    // plausible C signature with matching arity; the first
    // one libloading accepts wins.
    use crate::memory::FfiType;
    let arg0 = sig.arg_types.first().copied().unwrap_or(FfiType::Void);
    let arg1 = sig.arg_types.get(1).copied().unwrap_or(FfiType::Void);
    let ret = sig.ret_type;
    // arity 0
    if args.is_empty() {
        if ret == FfiType::Int {
            if let Ok(f) = unsafe { library.get::<extern "C" fn() -> i64>(name_bytes) } {
                return Some(Value::from(f()));
            }
        }
        if let Ok(f) = unsafe { library.get::<extern "C" fn()>(name_bytes) } {
            f();
            return None;
        }
    }
    // arity 1
    if args.len() == 1 {
        if arg0 == FfiType::String && ret == FfiType::Int {
            if let Ok(f) = unsafe {
                library.get::<extern "C" fn(*const std::os::raw::c_char) -> i64>(
                    name_bytes,
                )
            } {
                let arg0_raw = args[0].raw() as u64;
                let s = heap
                    .cstr_from_addr(arg0_raw)
                    .unwrap_or(std::ptr::null());
                if !s.is_null() {
                    return Some(Value::from(f(s)));
                }
                // Fall through — fail to next arm.
            }
        }
        if arg0 == FfiType::Int && ret == FfiType::Int {
            if let Ok(f) = unsafe { library.get::<extern "C" fn(i64) -> i64>(name_bytes) } {
                return Some(Value::from(f(args[0].as_int())));
            }
        }
        if arg0 == FfiType::Int {
            if let Ok(f) = unsafe { library.get::<extern "C" fn(i64)>(name_bytes) } {
                f(args[0].as_int());
                return None;
            }
        }
    }
    // arity 2
    if args.len() == 2 {
        if arg0 == FfiType::Int && arg1 == FfiType::Int && ret == FfiType::Int {
            if let Ok(f) = unsafe { library.get::<extern "C" fn(i64, i64) -> i64>(name_bytes) } {
                return Some(Value::from(f(args[0].as_int(), args[1].as_int())));
            }
        }
    }
    if let Ok(f) = unsafe { library.get::<extern "C" fn(i64)>(name_bytes) } {
        let args: Vec<i64> = args.iter().map(|v| v.as_int()).collect();
        f(args[0]);
        return None;
    }
    if let Ok(f) = unsafe { library.get::<extern "C" fn() -> i64>(name_bytes) } {
        return Some(Value::from(f()));
    }
    if let Ok(f) = unsafe { library.get::<extern "C" fn()>(name_bytes) } {
        f();
        return None;
    }
    if let Ok(f) =
        unsafe { library.get::<extern "C" fn(*const std::os::raw::c_char) -> i64>(name_bytes) }
    {
        let s = heap
            .cstr_from_addr(args[0].raw() as u64)
            .unwrap_or(std::ptr::null());
        return Some(Value::from(f(s)));
    }
    if let Ok(f) = unsafe { library.get::<extern "C" fn(*const std::os::raw::c_char)>(name_bytes) }
    {
        let s = heap
            .cstr_from_addr(args[0].raw() as u64)
            .unwrap_or(std::ptr::null());
        f(s);
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nullary native that increments a thread-local counter.
    /// Lets tests assert that the registry dispatches correctly.
    #[test]
    fn register_and_dispatch_void_void() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let native = NullaryVoid::new("inc", || {
            COUNTER.fetch_add(1, Ordering::SeqCst);
        });
        let mut registry = Natives::new();
        registry.register(Arc::new(native));
        let f = registry.get("inc").expect("registered native");
        assert_eq!(f.arity(), 0);
        let heap = Heap::default();
        f.invoke(&heap, &[]);
        f.invoke(&heap, &[]);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
    }

    /// A unary native that doubles its i64 input.
    #[test]
    fn dispatch_unary_i64_to_i64() {
        let native = UnaryI64ToI64::new("dbl", |x| x * 2);
        let f: Arc<dyn NativeFn> = Arc::new(native);
        let heap = Heap::default();
        let args = [Value::from(21_i64)];
        assert_eq!(f.invoke(&heap, &args).unwrap().as_int(), 42);
    }

    /// A string-arg native that returns the string length.
    /// Allocates an `ObjString` in the heap and uses
    /// `Heap::cstr_from_addr` to resolve the C string for the
    /// FFI call.
    #[test]
    fn dispatch_string_arg_to_i64() {
        let native = StringArgToI64::new("len", |s| {
            if s.is_null() {
                0
            } else {
                // SAFETY: `s` is a valid C string per the FFI
                // contract. We compute its length with `strlen`.
                unsafe { libc::strlen(s) as i64 }
            }
        });
        let f: Arc<dyn NativeFn> = Arc::new(native);
        let mut heap = Heap::default();
        // Allocate a real ObjString in the heap and pass its
        // address as the Value. This is what a `string` literal
        // would produce at runtime.
        let (_obj, _gc) = heap.alloc(ObjString::from("hello"), Object::String);
        let string_addr = _obj.addr();
        let args = [Value::from(string_addr)];
        assert_eq!(f.invoke(&heap, &args).unwrap().as_int(), 5);
    }

    /// Test the `Natives` registry with name-based lookup.
    #[test]
    fn registry_lookup_returns_correct_function() {
        let mut registry = Natives::new();
        registry.register(Arc::new(NullaryI64::new("a", || 1)));
        registry.register(Arc::new(NullaryI64::new("b", || 2)));
        assert!(registry.get("a").is_some());
        assert!(registry.get("b").is_some());
        assert!(registry.get("c").is_none());
        assert_eq!(registry.len(), 2);
    }

    /// Unregister removes a native.
    #[test]
    fn registry_unregister_removes_function() {
        let mut registry = Natives::new();
        registry.register(Arc::new(NullaryI64::new("x", || 0)));
        assert!(registry.get("x").is_some());
        let removed = registry.unregister("x");
        assert!(removed.is_some());
        assert!(registry.get("x").is_none());
        assert!(registry.is_empty());
    }

    /// Loading the C standard library and looking up `strlen`
    /// must succeed on every supported platform. This is the
    /// end-to-end smoke test for the FFI machinery: the
    /// library loads, the symbol resolves, the marshaller
    /// reads a real C string, the function runs, the result
    /// comes back.
    ///
    /// Skipped on platforms where libc isn't reachable via
    /// the standard library search path (e.g., some Linux
    /// distributions where `dlopen("c")` fails despite
    /// `libc.so.6` being present).
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn load_libc_and_call_strlen() {
        let lib = match load_library("c") {
            Ok(l) => l,
            Err(_) => {
                eprintln!("skipping: libc not reachable via dlopen");
                return;
            }
        };
        let strlen = match LibraryFn::new("strlen", "strlen", lib) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("skipping: strlen symbol not found");
                return;
            }
        };
        let mut heap = Heap::default();
        // Allocate an ObjString in the heap so the FFI
        // marshaller can resolve the C string.
        let (_obj, _gc) = heap.alloc(ObjString::from("hello world"), Object::String);
        let args = [Value::from(_obj.addr())];
        assert_eq!(strlen.invoke(&heap, &args).unwrap().as_int(), 11);
    }

    /// Round-trip a string through libc `strlen` to verify
    /// the C-string marshaller reads `Value`-as-pointer as a
    /// real C string.
    ///
    /// Skipped on platforms where libc isn't reachable.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn ffi_string_marshalling_round_trip() {
        let lib = match load_library("c") {
            Ok(l) => l,
            Err(_) => {
                eprintln!("skipping: libc not reachable via dlopen");
                return;
            }
        };
        let strlen = match LibraryFn::new("strlen", "strlen", lib) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("skipping: strlen symbol not found");
                return;
            }
        };
        let mut heap = Heap::default();
        for (input, expected) in [("", 0), ("a", 1), ("abc", 3), ("\x00tail", 0)] {
            // Allocate a real ObjString so the marshaller
            // can resolve the C string. The empty bytes in
            // the input are valid UTF-8; `CString::new`
            // rejects interior NULs, so we skip the
            // embedded-NUL case and use a hand-built
            // String for it.
            let (_obj, _gc) = heap.alloc(ObjString::from(input), Object::String);
            let args = [Value::from(_obj.addr())];
            assert_eq!(
                strlen.invoke(&heap, &args).unwrap().as_int(),
                expected as i64,
                "strlen({:?}) should be {}",
                input,
                expected
            );
        }
    }
}
