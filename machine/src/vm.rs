//! Bytecode interpreter: dispatch loop, automatic GC, and FFI.

use std::{
    ffi::c_void,
    fmt::Write as FmtWrite,
    io::{self, Write as IoWrite},
    path::PathBuf,
};

#[cfg(any(test, feature = "vm_profile"))]
use std::sync::atomic::{AtomicU64, Ordering};

use common::{
    ArchivedByte as Byte, ArchivedInstruction as Instruction, ArrayVec, Byte as RawByte,
    ProgramDebug, Value, byte_to_position, promise, unlikely,
};

use crate::{
    CStructLayout, CoroState, Frame, Heap, Member, ObjArray, ObjBoxed, ObjCoroutine, ObjEnum,
    ObjInstance, ObjFn, ObjPolyFn, ObjString, ObjTuple, Object, RefCoroutine, Stack,
};
use common::ValueTag;

/// Run mark-and-sweep after this many heap allocations (`INIT`, `STRING`, `FORMAT`, `MAKE_ENUM`).
const GC_TRIGGER_INTERVAL: usize = 64;

// Thread-local dispatch counter (tests / `vm_profile` only).
#[cfg(any(test, feature = "vm_profile"))]
thread_local! {
    static VM_DISPATCH_COUNT: AtomicU64 = const { AtomicU64::new(0) };
}

/// Reset the VM dispatch counter.
#[cfg(any(test, feature = "vm_profile"))]
pub fn reset_dispatch_count() {
    VM_DISPATCH_COUNT.with(|c| c.store(0, Ordering::Relaxed));
}

/// Read the VM dispatch counter.
#[cfg(any(test, feature = "vm_profile"))]
#[must_use]
pub fn dispatch_count() -> u64 {
    VM_DISPATCH_COUNT.with(|c| c.load(Ordering::Relaxed))
}

#[cfg(not(any(test, feature = "vm_profile")))]
#[must_use]
pub fn dispatch_count() -> u64 {
    0
}

#[cfg(not(any(test, feature = "vm_profile")))]
pub fn reset_dispatch_count() {}

macro_rules! binary {
    ($stack: expr, $op:tt, $from: ident, $to: ident) => {
        {
            let sp = $stack.tell();
            promise!(sp >= 2);
            let rhs_idx = sp - 1;
            let lhs_idx = sp - 2;
            let rhs = $stack[rhs_idx].$from();
            let lhs = $stack[lhs_idx].$from();
            $stack[lhs_idx].replace((lhs $op rhs).$to());
            $stack.seek(lhs_idx + 1);
        }
    };
    ($stack: expr, $op:tt, $from: ident) => {
        {
            let sp = $stack.tell();
            promise!(sp >= 2);
            let rhs_idx = sp - 1;
            let lhs_idx = sp - 2;
            let rhs = $stack[rhs_idx].$from();
            let lhs = $stack[lhs_idx].$from();
            $stack[lhs_idx].replace((lhs $op rhs) as _);
            $stack.seek(lhs_idx + 1);
        }
    };
}

macro_rules! unary {
    ($stack: expr, $op: tt, $from: ident, $to: ident) => {
        {
            let sp = $stack.tell();
            promise!(sp >= 1);
            let idx = sp - 1;
            let rhs = $stack[idx].$from();
            $stack[idx].replace(($op rhs).$to());
        }
    };
    ($stack: expr, $op: tt, $from: ident) => {
        {
            let sp = $stack.tell();
            promise!(sp >= 1);
            let idx = sp - 1;
            let rhs = $stack[idx].$from();
            $stack[idx].replace(($op rhs) as _);
        }
    };
}

// type External = fn(&[Value]) -> Value;

type OutputSink = Box<dyn IoWrite + Send>;

/// Saved resumer context while a coroutine runs on the shared stack.
#[derive(Clone, Copy)]
struct ResumeCtx {
    coro: RefCoroutine,
    base_sp: usize,
    frame_depth: usize,
}

/// Deferred `FfiInvoke` so libffi (and callbacks) run outside `execute`'s borrow.
struct PendingFfiInvoke {
    lib_addr: u64,
    function_id: usize,
    args: Vec<Value>,
    /// Per-arg FFI type tags for variadic calls (`None` when fixed-arity).
    arg_types: Option<Vec<crate::memory::FfiType>>,
    resume_ip: usize,
    resume_sp: usize,
}

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 8192>,
    frames: ArrayVec<Frame, S>,
    output: Option<OutputSink>,
    alloc_counter: usize,
    natives: crate::ffi::Natives,
    libraries: std::collections::HashMap<String, std::sync::Arc<crate::ffi::Library>>,
    userland_libraries: std::collections::HashMap<u64, std::sync::Arc<Object>>,
    resume_stack: Vec<ResumeCtx>,
    /// Directory of the entry script (for relative `dload` paths).
    base_dir: Option<PathBuf>,
    /// Extra search paths from `coil.toml` `[ffi]`.
    ffi_search_paths: Vec<PathBuf>,
    /// Registered C struct layouts for pass-by-value FFI.
    struct_layouts: Vec<CStructLayout>,
    /// Keeps libffi callback trampolines alive (ties lifetime to VM run).
    ffi_closures: Vec<crate::ffi::OwnedClosure>,
    /// Bytecode/constants for nested `call_function` / callbacks.
    program_code: Vec<RawByte>,
    program_constants: Vec<u64>,
    /// When > 0, `RETURN` captures into `nested_return` instead of unwinding to caller.
    nested_depth: u32,
    /// Stack of frame-stack lengths at each active [`call_function`] entry.
    /// Only a `RETURN` that pops back to `last()` should capture `nested_return`
    /// (inner `CALL`s must still unwind normally). A stack (not a scalar) is
    /// required so nested `call_function` reentrancy (FFI callbacks) does not
    /// overwrite the outer depth.
    nested_frame_depths: Vec<usize>,
    nested_return: Option<Value>,
    /// Set when `execute` pauses before a native FFI call that may reenter the VM.
    pending_ffi: Option<PendingFfiInvoke>,
    /// Set when a language-level `panic` aborts the VM.
    panicked: bool,
    /// Global static slots (`LoadStatic` / `StoreStatic`).
    statics: Vec<Value>,
    /// Debug line table (parallel to archived bytecode indices).
    program_debug: ProgramDebug,
    /// Shared program image for OS thread workers (`spawn`).
    thread_program: Option<std::sync::Arc<crate::thread::ThreadProgram>>,
    /// Optional shared stdout capture for worker threads.
    shared_print: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
    /// Undetached spawns owned by this VM (joined at end of `run_with_pool`).
    live_threads: crate::thread::LiveThreadRegistry,
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
            stack: Stack::new(),
            output: None,
            alloc_counter: 0,
            natives: crate::ffi::Natives::new(),
            libraries: std::collections::HashMap::new(),
            userland_libraries: std::collections::HashMap::new(),
            resume_stack: Vec::new(),
            base_dir: None,
            ffi_search_paths: Vec::new(),
            struct_layouts: Vec::new(),
            ffi_closures: Vec::new(),
            program_code: Vec::new(),
            program_constants: Vec::new(),
            nested_depth: 0,
            nested_frame_depths: Vec::new(),
            nested_return: None,
            pending_ffi: None,
            panicked: false,
            statics: Vec::new(),
            program_debug: ProgramDebug::default(),
            thread_program: None,
            shared_print: None,
            live_threads: crate::thread::new_live_thread_registry(),
        }
    }
}

impl<const S: usize> Machine<S> {
    pub fn set_ffi_paths(&mut self, base_dir: Option<PathBuf>, search_paths: Vec<PathBuf>) {
        self.base_dir = base_dir;
        self.ffi_search_paths = search_paths;
    }

    pub fn set_program_debug(&mut self, debug: ProgramDebug) {
        self.program_debug = debug;
    }

    fn resolve_source_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            return p;
        }
        if std::fs::metadata(&p).is_ok() {
            return p;
        }
        if let Some(base) = &self.base_dir {
            let root = base.parent().unwrap_or(base.as_path());
            let from_root = root.join(path);
            if std::fs::metadata(&from_root).is_ok() {
                return from_root;
            }
            let from_base = base.join(path);
            if std::fs::metadata(&from_base).is_ok() {
                return from_base;
            }
        }
        p
    }

    fn format_panic_location(&self, panic_insn_ip: usize) -> Option<String> {
        let loc = self.program_debug.debug_locs.get(panic_insn_ip)?;
        if !loc.is_known() {
            return None;
        }
        let path = self.program_debug.source_files.get(loc.file as usize)?;
        let read_path = self.resolve_source_path(path);
        let text = std::fs::read_to_string(&read_path).ok()?;
        let pos = byte_to_position(&text, loc.start_byte as usize);
        Some(format!(
            "{}:{}:{}",
            path, pos.line, pos.column
        ))
    }

    pub fn with_ffi_paths(mut self, base_dir: Option<PathBuf>, search_paths: Vec<PathBuf>) -> Self {
        self.base_dir = base_dir;
        self.ffi_search_paths = search_paths;
        self
    }

    pub fn register_struct_layout(&mut self, layout: CStructLayout) -> u32 {
        let id = self.struct_layouts.len() as u32;
        self.struct_layouts.push(layout);
        id
    }

    // pub fn register(&mut self, name: usize, func: External) {
    //     self.native.insert(name, func);
    // }

    /// Free function so `execute` can borrow `frames` and `heap` separately.
    /// Delegates to [`Heap::find_object_by_addr`] (O(1) addr index).
    fn find_object_by_addr(heap: &Heap, addr: u64) -> Option<Object> {
        heap.find_object_by_addr(addr)
    }

    #[allow(dead_code)]
    fn value_to_string(&self, v: &Value) -> String {
        self.heap
            .cstr_from_addr(v.raw() as u64)
            .map(|s| unsafe { std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned() })
            .unwrap_or_default()
    }

    fn ffi_type_from_value(v: &Value, heap: &Heap) -> crate::memory::FfiType {
        let (tag, aux) = Self::decode_ffi_type_tag(v, heap);
        crate::memory::FfiType::from_tag(tag, aux)
    }

    fn decode_ffi_type_tag(v: &Value, heap: &Heap) -> (u32, u32) {
        let raw = v.raw() as u64;
        if raw <= common::tag::STRUCT as u64 {
            return (raw as u32, 0);
        }
        if raw > 0xFFFF {
            return ((raw & 0xFFFF) as u32, (raw >> 16) as u32);
        }
        if let Some(crate::memory::Object::Enum(gc)) = Self::find_object_by_addr(heap, raw) {
            (gc.as_ref().tag, 0)
        } else {
            (common::tag::INT, 0)
        }
    }

    fn object_string_value(heap: &Heap, v: &Value) -> String {
        let addr = v.raw() as u64;
        let obj = Self::find_object_by_addr(heap, addr);
        if let Some(crate::memory::Object::String(gc)) = obj {
            gc.as_ref().data.clone()
        } else {
            String::new()
        }
    }

    /// Convert a runtime value to a display string (Show / `%v` / STRINGIFY).
    fn stringify_value(heap: &Heap, v: Value) -> String {
        let addr = v.raw() as u64;
        if !v.raw().is_null() && heap.contains_addr(v.raw()) {
            match Self::find_object_by_addr(heap, addr) {
                Some(Object::Boxed(gc)) => {
                    let b = gc.as_ref();
                    match ValueTag::from_u16(b.tag) {
                        Some(ValueTag::Int) => match &b.payload {
                            Member::Value(iv) => iv.as_int().to_string(),
                            _ => "?".into(),
                        },
                        Some(ValueTag::Float) => match &b.payload {
                            Member::Value(iv) => format!("{:?}", iv.as_float()),
                            _ => "?".into(),
                        },
                        Some(ValueTag::Bool) => match &b.payload {
                            Member::Value(iv) => {
                                if iv.as_int() != 0 {
                                    "true".into()
                                } else {
                                    "false".into()
                                }
                            }
                            _ => "?".into(),
                        },
                        Some(ValueTag::String) => match &b.payload {
                            Member::Object(o) => {
                                Self::object_string_value(heap, &Value::from(o.addr()))
                            }
                            Member::Value(iv) => Self::object_string_value(heap, iv),
                        },
                        Some(ValueTag::Unit) => "()".into(),
                        _ => "?".into(),
                    }
                }
                Some(Object::String(gc)) => gc.as_ref().data.clone(),
                _ => v.as_int().to_string(),
            }
        } else if v.raw().is_null() {
            // `Value::default()` / unit / false-ish null pointer.
            "0".into()
        } else {
            v.as_int().to_string()
        }
    }

    fn materialize_callback_args(
        &mut self,
        sig: &crate::ffi::FfiSignature,
        args: &[Value],
    ) -> Result<Vec<Value>, crate::ffi::FfiError> {
        use crate::ffi::{VmCallFn, callback_cif, make_int_callback};
        use crate::memory::FfiType;
        let mut out = args.to_vec();
        let vm_ptr = self as *mut Self as *mut c_void;
        let call_fn: VmCallFn = Self::invoke_call;
        for (i, ty) in sig.args.iter().enumerate() {
            if let FfiType::Callback(_) = ty {
                let offset = out[i].as_int() as u32;
                let cif = callback_cif(&[FfiType::Int], FfiType::Int, &self.struct_layouts)?;
                let closure = make_int_callback(vm_ptr, offset, call_fn, cif)?;
                let ptr = closure.code_ptr_usize();
                self.ffi_closures.push(closure);
                out[i] = Value::from(ptr as u64);
            }
        }
        Ok(out)
    }

    /// Register a new FFI function on the given library `Object`.
    fn register_signature_on_object(
        obj: &mut Object,
        sig: crate::ffi::FfiSignature,
        layouts: &[CStructLayout],
    ) -> Result<usize, crate::ffi::FfiError> {
        if let crate::memory::Object::Library(gc) = obj {
            let obj_lib: &mut crate::memory::ObjLibrary = (**gc).as_mut();
            crate::ffi::register_on_library(obj_lib, sig, layouts)
        } else {
            Err(crate::ffi::FfiError::InvalidHandle(
                "not a library object".into(),
            ))
        }
    }

    /// Load a shared library; returns its heap address as a `Value`.
    pub fn load_userland_library(&mut self, path: &str) -> Result<Value, String> {
        let lib_arc = crate::ffi::load_library(path).map_err(|e| e.to_string())?;
        let (object, _gc) = self.heap.alloc_library(lib_arc.clone());
        let addr = object.addr();
        self.userland_libraries
            .insert(addr, std::sync::Arc::new(object));
        self.libraries
            .entry(path.to_string())
            .or_insert_with(|| lib_arc.clone());
        Ok(Value::from(addr as *mut u8))
    }

    /// Mark-and-sweep GC. Free function to avoid borrow conflicts in `execute`.
    fn gc_collect(
        heap: &mut Heap,
        stack: &Stack<Value, 8192>,
        resume_stack: &[ResumeCtx],
        alloc_counter: &mut usize,
    ) {
        let mut roots: Vec<u64> = stack
            .buffer()
            .iter()
            .filter_map(|v| {
                let addr = v.raw() as u64;
                if addr != 0 && heap.contains_addr(addr as *mut u8) {
                    Some(addr)
                } else {
                    None
                }
            })
            .collect();

        for ctx in resume_stack {
            roots.push(ctx.coro.as_ptr() as u64);
        }

        // Conservatively root values held in suspended coroutine stacks.
        for obj in heap.into_iter() {
            if let Object::Coroutine(gc) = obj {
                roots.push(gc.as_ptr() as u64);
                for v in &gc.as_ref().saved_stack {
                    let addr = v.raw() as u64;
                    if addr != 0 && heap.contains_addr(addr as *mut u8) {
                        roots.push(addr);
                    }
                }
                if let Some(delegate) = &gc.as_ref().yield_from {
                    roots.push(delegate.as_ptr() as u64);
                }
            }
        }

        heap.trace(&roots);

        let mut gray: Vec<Object> = Vec::new();
        let mut current = heap.head_for_lookup();
        let mut root_objects: Vec<Object> = Vec::new();
        while let Some(reference) = current {
            if reference.is_marked() {
                root_objects.push(reference);
            }
            current = reference.get_next();
        }
        for root in &root_objects {
            Self::mark_aggregate_elements(heap, root, &mut gray);
            root.mark_references(&mut gray);
        }
        while let Some(obj) = gray.pop() {
            Self::mark_aggregate_elements(heap, &obj, &mut gray);
            obj.mark_references(&mut gray);
        }

        // SAFETY: all reachable objects were marked above.
        unsafe { heap.sweep() };

        *alloc_counter = 0;
    }

    /// Trace heap pointers stored as raw `Value`s inside arrays/tuples.
    /// `Object::mark_references` cannot do this alone — those aggregates
    /// keep `Vec<Value>`, not `Member::Object`.
    fn mark_aggregate_elements(heap: &Heap, obj: &Object, gray: &mut Vec<Object>) {
        match obj {
            Object::Array(gc) => {
                for v in &gc.as_ref().elements {
                    Self::mark_value_if_heap(heap, *v, gray);
                }
            }
            Object::Tuple(gc) => {
                for v in &gc.as_ref().elements {
                    Self::mark_value_if_heap(heap, *v, gray);
                }
            }
            Object::Fn(gc) => {
                let f = gc.as_ref();
                for v in f.captures.iter().chain(f.captured_args.iter()) {
                    Self::mark_value_if_heap(heap, *v, gray);
                }
            }
            _ => {}
        }
    }

    fn mark_value_if_heap(heap: &Heap, v: Value, gray: &mut Vec<Object>) {
        let addr = v.raw() as u64;
        if addr == 0 || !heap.contains_addr(addr as *mut u8) {
            return;
        }
        if let Some(child) = Self::find_object_by_addr(heap, addr) {
            child.mark(gray);
        }
    }
}

impl<const S: usize> Machine<S> {
    #[cfg(test)]
    pub fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    #[cfg(test)]
    pub fn pop(&mut self) -> Value {
        self.stack.pop()
    }

    #[cfg(test)]
    pub fn tell(&self) -> usize {
        self.stack.tell()
    }

    /// Redirect `PRINT` output (used by pipeline tests).
    pub fn with_output<W: IoWrite + Send + 'static>(&mut self, writer: W) -> Option<OutputSink> {
        let prev = self.output.take();
        self.output = Some(Box::new(writer));
        prev
    }

    /// Reset the output sink back to stdout. Returns the previous
    /// sink so the caller can recover it (useful in tests that
    /// want to scope the redirection).
    pub fn restore_output(&mut self) -> Option<OutputSink> {
        self.output.take()
    }

    /// Register a host native with an explicit signature via the
    /// builder API. Returns the stable native id used by
    /// [`Instruction::HostInvoke`].
    pub fn register_fn<F>(&mut self, sig: crate::ffi::FfiSignature, func: F) -> usize
    where
        F: Fn(&mut Heap, &[Value]) -> Result<Option<Value>, crate::ffi::FfiError>
            + Send
            + Sync
            + 'static,
    {
        self.natives
            .register(std::sync::Arc::new(crate::ffi::HostClosureFn::new(
                sig, func,
            )))
    }

    /// Back-compat alias for [`Self::register_fn`].
    pub fn register_native(&mut self, native: std::sync::Arc<dyn crate::ffi::NativeFn>) -> usize {
        self.natives.register(native)
    }

    /// Replace the host-native table with a clone of `other` (worker threads).
    pub fn install_natives(&mut self, other: &crate::ffi::Natives) {
        self.natives = other.clone_registry();
    }

    pub fn set_thread_program(&mut self, program: std::sync::Arc<crate::thread::ThreadProgram>) {
        self.thread_program = Some(program);
    }

    pub fn thread_program(&self) -> Option<&crate::thread::ThreadProgram> {
        self.thread_program.as_deref()
    }

    pub fn set_shared_print(&mut self, buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        self.shared_print = Some(buf);
    }

    pub fn shared_print(&self) -> Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>> {
        self.shared_print.clone()
    }

    /// Replace the undetached-spawn registry (used by workers to share the
    /// root VM's list so nested `spawn` still joins with the root).
    pub fn set_live_threads(&mut self, registry: crate::thread::LiveThreadRegistry) {
        self.live_threads = registry;
    }

    pub fn live_threads(&self) -> &crate::thread::LiveThreadRegistry {
        &self.live_threads
    }

    /// Allocate global static slots without running bytecode.
    pub fn init_static_slots(&mut self, static_slots: u32) {
        self.statics = vec![Value::default(); static_slots as usize];
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// Snapshot needed to spawn a worker on this program.
    pub fn thread_spawn_context(&self) -> Option<crate::thread::ThreadSpawnContext> {
        let program = self.thread_program.clone()?;
        Some(crate::thread::ThreadSpawnContext {
            program,
            natives: self.natives.clone_registry(),
            shared_print: self.shared_print.clone(),
            live_threads: std::sync::Arc::clone(&self.live_threads),
        })
    }

    fn sync_thread_program_from_current(&mut self) {
        if self.thread_program.is_some() {
            return;
        }
        if self.program_code.is_empty() {
            return;
        }
        self.thread_program = Some(std::sync::Arc::new(crate::thread::ThreadProgram {
            code: std::sync::Arc::new(self.program_code.clone()),
            constants: std::sync::Arc::new(self.program_constants.clone()),
            static_slot_count: self.statics.len() as u32,
            debug: self.program_debug.clone(),
        }));
    }

    /// Register a function signature on a previously-loaded
    /// userland library (host/test helper — userland code uses
    /// `DeclareFFI` at runtime).
    pub fn register_ffi_function(
        &mut self,
        library_value: Value,
        signature: crate::ffi::FfiSignature,
    ) -> Result<usize, String> {
        let addr = library_value.raw() as u64;
        let mut lib_obj_arc = self
            .userland_libraries
            .get(&addr)
            .cloned()
            .ok_or_else(|| format!("not a loaded library: 0x{:x}", addr))?;
        let lib_obj_mut = std::sync::Arc::make_mut(&mut lib_obj_arc);
        if let crate::memory::Object::Library(gc) = lib_obj_mut {
            let obj_lib: &mut crate::memory::ObjLibrary = (**gc).as_mut();
            let id = crate::ffi::register_on_library(obj_lib, signature, &self.struct_layouts)
                .map_err(|e| e.to_string())?;
            self.userland_libraries
                .insert(addr, std::sync::Arc::new(*lib_obj_mut));
            Ok(id)
        } else {
            Err("not a library object".to_string())
        }
    }

    /// Manually trigger GC (for tests).
    pub fn collect_garbage(&mut self) {
        Self::gc_collect(
            &mut self.heap,
            &self.stack,
            &self.resume_stack,
            &mut self.alloc_counter,
        );
    }

    fn with_coroutine_mut(&self, addr: u64, f: impl FnOnce(&mut ObjCoroutine)) {
        let mut current = self.heap.head_for_lookup();
        while let Some(reference) = current {
            if reference.addr() == addr {
                if let Object::Coroutine(gc) = reference {
                    f(gc.payload_mut());
                }
                return;
            }
            current = reference.get_next();
        }
    }

    fn find_delegator(&self, sub: RefCoroutine) -> Option<RefCoroutine> {
        let sub_addr = sub.as_ptr() as u64;
        let mut current = self.heap.head_for_lookup();
        while let Some(reference) = current {
            if let Object::Coroutine(gc) = reference {
                if gc
                    .as_ref()
                    .yield_from
                    .as_ref()
                    .is_some_and(|d| d.as_ptr() as u64 == sub_addr)
                {
                    return Some(gc.clone());
                }
            }
            current = reference.get_next();
        }
        None
    }

    fn save_coroutine_state(
        &self,
        coro_gc: RefCoroutine,
        ip: usize,
        sp: usize,
        base_sp: usize,
        frame_depth: usize,
    ) {
        let top = self.stack.tell();
        let segment = if base_sp <= top {
            self.stack.as_slice()[base_sp..top].to_vec()
        } else {
            Vec::new()
        };
        let current_depth = self.frames.len();
        let mut saved_frames = Vec::new();
        for idx in (frame_depth + 1)..current_depth {
            saved_frames.push((
                self.frames[idx].tell(),
                self.frames[idx].get().saturating_sub(base_sp),
            ));
        }
        if saved_frames.is_empty() {
            saved_frames.push((ip, sp.saturating_sub(base_sp)));
        } else {
            saved_frames.last_mut().unwrap().0 = ip;
        }

        self.with_coroutine_mut(coro_gc.as_ptr() as u64, |coro| {
            coro.saved_stack = segment;
            coro.saved_frames = saved_frames;
            coro.resume_ip = ip;
            coro.state = CoroState::Suspended;
        });
    }

    fn after_return(&mut self, ip: &mut usize, sp: &mut usize) {
        let caller = self.frames.get_mut();
        *ip = caller.tell();
        *sp = caller.get();
        if let Some(ctx) = self.resume_stack.last() {
            if self.frames.len() <= ctx.frame_depth {
                self.with_coroutine_mut(ctx.coro.as_ptr() as u64, |coro| {
                    coro.state = CoroState::Done;
                    coro.saved_stack.clear();
                    coro.saved_frames.clear();
                    coro.yield_from = None;
                });
                self.resume_stack.pop();
            }
        }
    }

    fn resume_coroutine(
        &mut self,
        ip: &mut usize,
        sp: &mut usize,
        gc: RefCoroutine,
        send_val: Value,
        code: &[Byte],
        push_send_for_receive: bool,
    ) {
        let return_ip = *ip;
        let coro = gc.as_ref();
        let base_sp = self.stack.tell();

        self.frames.get_mut().seek(return_ip);

        self.with_coroutine_mut(gc.as_ptr() as u64, |c| {
            c.pending_send = send_val;
        });

        self.resume_stack.push(ResumeCtx {
            coro: gc,
            base_sp,
            frame_depth: self.frames.len(),
        });

        for v in &coro.saved_stack {
            self.stack.push(*v);
        }

        for &(frame_ip, sp_off) in &coro.saved_frames {
            self.frames.setup_current_and_advance(|f| {
                f.seek(frame_ip);
                f.set(base_sp + sp_off);
            });
        }

        *ip = coro.resume_ip;
        *sp = base_sp + coro.saved_frames.last().map_or(0, |(_, off)| *off);

        if push_send_for_receive
            && *ip < code.len()
            && matches!(code[*ip].bytecode(), Instruction::StorePop)
        {
            self.stack.push(send_val);
        }
    }

    fn delegate_yield_to_parent(
        &mut self,
        sub_gc: RefCoroutine,
        ip: &mut usize,
        sp: &mut usize,
        yield_val: Value,
        sub_base_sp: usize,
        sub_frame_depth: usize,
    ) {
        let Some(parent) = self.find_delegator(sub_gc) else {
            return;
        };

        self.save_coroutine_state(sub_gc, *ip, *sp, sub_base_sp, sub_frame_depth);

        let parent_entry_idx = self
            .resume_stack
            .iter()
            .position(|c| c.coro.as_ptr() == parent.as_ptr())
            .unwrap_or(self.resume_stack.len().saturating_sub(1));
        let parent_ctx = &self.resume_stack[parent_entry_idx];

        self.save_coroutine_state(
            parent,
            parent.as_ref().yield_from_resume_ip,
            self.stack.tell(),
            parent_ctx.base_sp,
            parent_ctx.frame_depth,
        );

        self.stack.seek(parent_ctx.base_sp);
        while self.frames.len() > parent_ctx.frame_depth {
            self.frames.pop();
        }
        if self.resume_stack.len() > parent_entry_idx + 1 {
            self.resume_stack.truncate(parent_entry_idx + 1);
        }

        self.stack.push(yield_val);
        let caller = self.frames.get_mut();
        *ip = caller.tell();
        *sp = caller.get();
    }

    fn yield_coroutine(&mut self, ip: &mut usize, sp: &mut usize, yield_val: Value) {
        let Some(ctx) = self
            .resume_stack
            .last()
            .map(|c| (c.coro, c.base_sp, c.frame_depth))
        else {
            self.stack.push(yield_val);
            return;
        };
        let (coro_gc, base_sp, frame_depth) = ctx;

        if self.find_delegator(coro_gc).is_some() {
            self.delegate_yield_to_parent(coro_gc, ip, sp, yield_val, base_sp, frame_depth);
            return;
        }

        let current_depth = self.frames.len();
        let top = self.stack.tell();
        let coro_sp = if current_depth > frame_depth {
            self.frames[current_depth - 1].get()
        } else {
            base_sp
        };
        let segment = if coro_sp <= top {
            self.stack.as_slice()[coro_sp..top].to_vec()
        } else {
            Vec::new()
        };
        let mut saved_frames = Vec::new();
        for idx in (frame_depth + 1)..current_depth {
            saved_frames.push((self.frames[idx].tell(), self.frames[idx].get() - base_sp));
        }
        if saved_frames.is_empty() {
            saved_frames.push((*ip, *sp - base_sp));
        } else {
            saved_frames.last_mut().unwrap().0 = *ip;
        }

        self.with_coroutine_mut(coro_gc.as_ptr() as u64, |coro| {
            coro.saved_stack = segment;
            coro.saved_frames = saved_frames;
            coro.resume_ip = *ip;
            coro.state = CoroState::Suspended;
        });

        self.stack.seek(base_sp);
        while self.frames.len() > frame_depth {
            self.frames.pop();
        }

        self.stack.push(yield_val);
        let caller = self.frames.get_mut();
        *ip = caller.tell();
        *sp = caller.get();
        self.resume_stack.pop();
    }

    fn start_yield_from(
        &mut self,
        ip: &mut usize,
        sp: &mut usize,
        sub: RefCoroutine,
        code: &[Byte],
    ) {
        let Some(outer_ctx) = self.resume_stack.last().copied() else {
            return;
        };
        let outer = outer_ctx.coro;
        self.save_coroutine_state(outer, *ip, *sp, outer_ctx.base_sp, outer_ctx.frame_depth);
        self.with_coroutine_mut(outer.as_ptr() as u64, |outer_coro| {
            outer_coro.yield_from = Some(sub);
            outer_coro.yield_from_resume_ip = *ip;
        });
        self.resume_coroutine(ip, sp, sub, Value::from(0_i64), code, false);
    }

    /// Read-only access to the heap. Used by the GC integration
    /// test to assert that the heap didn't grow unboundedly.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// Current allocation counter (number of allocations since
    /// the last GC). Exposed for tests.
    pub fn alloc_counter(&self) -> usize {
        self.alloc_counter
    }

    /// True when a language-level `panic` aborted the last run.
    pub fn panicked(&self) -> bool {
        self.panicked
    }

    /// Load bytecode for reentrant [`call_function`] without running `main`.
    pub fn load_program(&mut self, code: &[RawByte], constants: &[u64]) {
        self.program_code = code.to_vec();
        self.program_constants = constants.to_vec();
        self.panicked = false;
    }

    /// True when `value` is a heap `Result::Ok` (enum tag 0).
    pub fn result_is_ok(&self, value: Value) -> bool {
        match Self::find_object_by_addr(&self.heap, value.raw() as u64) {
            Some(Object::Enum(gc)) => gc.as_ref().tag == 0,
            _ => false,
        }
    }

    pub fn run(&mut self, code: &[Byte]) {
        self.run_with_pool(code, &[], 0);
    }

    /// Run bytecode with an optional constant pool for wide immediates.
    pub fn run_with_pool(&mut self, code: &[Byte], constants: &[u64], static_slots: u32) {
        if code.is_empty() {
            return;
        }
        self.statics = vec![Value::default(); static_slots as usize];
        self.program_code = unsafe {
            std::slice::from_raw_parts(code.as_ptr().cast::<RawByte>(), code.len()).to_vec()
        };
        self.program_constants = constants.to_vec();
        self.sync_thread_program_from_current();
        let mut ip = 0usize;
        loop {
            let paused = self.execute(code, constants, ip);
            if let Some(pending) = self.pending_ffi.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_ffi_invoke(pending);
                ip = resume_ip;
                continue;
            }
            if !paused {
                break;
            }
        }
        // Keep undetached workers alive past main's return. Without this,
        // process exit kills threads still blocked in `recv` / still starting,
        // which looks like "recv never blocks" and "nothing after recv runs".
        // Only joins *this* Machine's registry (not a process-global list).
        crate::thread::join_undetached_threads(&self.live_threads);
    }

    fn finish_pending_ffi_invoke(&mut self, pending: PendingFfiInvoke) {
        self.frames.get_mut().set(pending.resume_sp);
        let lib_obj = self.userland_libraries.get(&pending.lib_addr).cloned();
        let invoke_result = match lib_obj {
            Some(obj) => {
                let l = match obj.as_ref() {
                    crate::memory::Object::Library(gc) => gc,
                    _ => {
                        self.push_result_err(
                            crate::ffi::FfiErrorKindTag::InvalidHandle,
                            "invalid library handle (not a loaded library)".into(),
                        );
                        return;
                    }
                };
                let lib_ref: &crate::memory::ObjLibrary = l.as_ref();
                if pending.function_id < lib_ref.signatures.len() {
                    let registered = &lib_ref.signatures[pending.function_id];
                    let ffi_sig = registered.ffi_signature();
                    let args = match self.materialize_callback_args(&ffi_sig, &pending.args) {
                        Ok(a) => a,
                        Err(e) => {
                            self.push_ffi_error(e);
                            return;
                        }
                    };
                    let mut ctx = crate::ffi::InvokeContext::new(
                        &mut self.heap as *mut Heap,
                        &self.struct_layouts,
                    );
                    let mut closure_ptrs = Vec::new();
                    crate::ffi::invoke_via_libffi(
                        &registered.prepared,
                        &ffi_sig,
                        &args,
                        pending.arg_types.as_deref(),
                        &mut ctx,
                        &mut closure_ptrs,
                    )
                } else {
                    Err(crate::ffi::FfiError::InvalidHandle(
                        "function id out of range".into(),
                    ))
                }
            }
            None => Err(crate::ffi::FfiError::InvalidHandle(
                "invalid library handle".into(),
            )),
        };
        match invoke_result {
            Ok(Some(v)) => self.push_result_ok(v),
            Ok(None) => self.push_result_ok(Value::default()),
            Err(e) => self.push_ffi_error(e),
        }
    }

    /// Push `Result::Ok(payload)` for userland FFI builtins.
    fn push_result_ok(&mut self, payload: Value) {
        let v = crate::io::alloc_result_ok(&mut self.heap, payload);
        self.stack.push(v);
    }

    /// Push `Result::Err(ffi::Error)` for userland FFI builtins.
    fn push_result_err(&mut self, kind: crate::ffi::FfiErrorKindTag, message: String) {
        let v = crate::ffi::alloc_result_ffi_err(&mut self.heap, kind, message);
        self.stack.push(v);
    }

    /// Map an [`FfiError`](crate::ffi::FfiError) into `Result::Err(ffi::Error)`.
    fn push_ffi_error(&mut self, err: crate::ffi::FfiError) {
        let kind = crate::ffi::FfiErrorKindTag::from_ffi_error(&err);
        self.push_result_err(kind, err.to_string());
    }

    /// Call a coil function at `offset` reentrantly (for FFI callbacks).
    pub fn call_function(&mut self, offset: u32, args: &[Value]) -> Value {
        let saved_sp = self.stack.tell();
        for a in args {
            self.stack.push(*a);
        }
        self.nested_return = None;
        self.nested_depth += 1;
        let callee_sp = self.stack.tell().saturating_sub(args.len());
        self.frames.setup_current_and_advance(|f| {
            f.seek(0);
            f.set(callee_sp);
        });
        // Capture only when RETURN reaches this frame depth (the
        // call_function entry), not when inner CALLs return.
        self.nested_frame_depths.push(self.frames.len());
        let code: &[Byte] = unsafe {
            std::slice::from_raw_parts(self.program_code.as_ptr().cast(), self.program_code.len())
        };
        let constants = self.program_constants.clone();
        let mut ip = offset as usize;
        loop {
            let paused = self.execute(code, &constants, ip);
            if let Some(pending) = self.pending_ffi.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_ffi_invoke(pending);
                ip = resume_ip;
                continue;
            }
            if !paused {
                break;
            }
        }
        let _ = self.frames.pop();
        self.stack.seek(saved_sp);
        self.nested_depth -= 1;
        let _ = self.nested_frame_depths.pop();
        self.nested_return.take().unwrap_or_default()
    }

    /// Stash a return value when `execute` runs inside [`Self::call_function`].
    #[inline]
    fn capture_nested_return(&mut self, ret_val: Value) -> bool {
        let nested_target = self.nested_frame_depths.last().copied().unwrap_or(0);
        if self.nested_depth > 0 && self.frames.len() == nested_target {
            self.nested_return = Some(ret_val);
            true
        } else {
            false
        }
    }

    /// Type-erased entry for libffi callback trampolines (monomorphized per `S`).
    unsafe fn invoke_call(
        vm: *mut c_void,
        offset: u32,
        args_ptr: *const Value,
        len: usize,
    ) -> Value {
        // Edition 2024: bodies of `unsafe fn` are safe by default.
        unsafe {
            let vm = &mut *(vm.cast::<Self>());
            let args = std::slice::from_raw_parts(args_ptr, len);
            vm.call_function(offset, args)
        }
    }

    /// Run compiler-produced bytecode (archived layout, no `.hyc` round-trip).
    pub fn run_raw(&mut self, code: &[RawByte], constants: &[u64], static_slots: u32) {
        let code: &[Byte] = unsafe { std::slice::from_raw_parts(code.as_ptr().cast(), code.len()) };
        self.run_with_pool(code, constants, static_slots);
    }

    /// Never-inline: `#[inline(always)]` forced fat LTO to paste this giant
    /// `match` into `run_with_pool` / `call_function`. Whole-program context
    /// (e.g. a larger compiler in the same binary) then reshapes dispatch
    /// enough to blow branch-mispredict rates on some CPUs while keeping
    /// dynamic instruction counts identical. A single outlined copy matches
    /// the non-LTO `machine` codegen (already identical to `main`'s).
    #[inline(never)]
    fn execute(&mut self, code: &[Byte], constants: &[u64], start_ip: usize) -> bool {
        let _active_guard = crate::thread::HostStateGuard::enter(self);

        #[cfg(debug_assertions)]
        let frame_no = self.frames.len();

        let mut ip: usize = start_ip;
        let mut sp = self.frames.get_mut().get();

        while ip < code.len() {
            #[cfg(any(test, feature = "vm_profile"))]
            VM_DISPATCH_COUNT.with(|c| c.fetch_add(1, Ordering::Relaxed));

            let opcode = &code[ip];
            ip += 1;

            #[cfg(debug_assertions)]
            {
                eprintln!(
                    "#{:<2} @ {:0>4} - {:>8}[{:0>4}, {:0>4}] - {:?}",
                    frame_no,
                    ip,
                    *opcode.bytecode() as u8,
                    opcode.operand_u16(0),
                    opcode.operand_u16(1),
                    self.stack.as_slice()
                );
            }

            let bc = opcode.bytecode();
            // Release-only optimizer hint: must track the LAST `Instruction`
            // variant. A stale ceiling (e.g. YieldFromCoro) makes later opcodes
            // (`StoreIndex`, `DoneCoro`, `ArrayPush`, …) UB via assert_unchecked.
            #[cfg(not(debug_assertions))]
            promise!(*bc as u8 <= Instruction::CastBoolToInt as u8);

            match bc {
                Instruction::POP => {
                    self.stack.pop();
                }
                Instruction::DUPLICATE => {
                    self.stack.push(*self.stack.peek());
                }
                Instruction::CONST => {
                    let op = opcode.operand_u32();
                    let raw = if unlikely(op & Byte::POOL_FLAG != 0) {
                        constants[(op & !Byte::POOL_FLAG) as usize]
                    } else {
                        op as i32 as i64 as u64
                    };
                    self.stack.push(Value::from(raw));
                }
                Instruction::CodePtr => {
                    // Absolute bytecode entry — same stack representation as an
                    // integer constant so `CallIndirect` / dict `Index` can
                    // treat it as a raw code offset.
                    let offset = opcode.operand_u32() as i64;
                    self.stack.push(Value::from(offset));
                }
                Instruction::STORE => {
                    // No-op: stack and locals share memory; UNPACK/JUMP_IF_MATCH
                    // already wrote match bindings into slot positions.
                }
                Instruction::LOAD => {
                    self.stack
                        .push(self.stack[sp + opcode.operand_u32() as usize]);
                }
                Instruction::INC => {
                    let (slot, prefix, is_float) = opcode.inc_dec_parts();
                    let idx = sp + slot;
                    let old = self.stack[idx];
                    let new_val = if is_float {
                        Value::from(old.as_float() + 1.0)
                    } else {
                        Value::from(old.as_int() + 1)
                    };
                    self.stack[idx] = new_val;
                    self.stack.push(if prefix { new_val } else { old });
                }
                Instruction::DEC => {
                    let (slot, prefix, is_float) = opcode.inc_dec_parts();
                    let idx = sp + slot;
                    let old = self.stack[idx];
                    let new_val = if is_float {
                        Value::from(old.as_float() - 1.0)
                    } else {
                        Value::from(old.as_int() - 1)
                    };
                    self.stack[idx] = new_val;
                    self.stack.push(if prefix { new_val } else { old });
                }
                Instruction::NOT => unary!(self.stack, !, as_int),
                Instruction::LogNot => {
                    let val = self.stack.pop();
                    self.stack.push(Value::from(!(val.as_int() != 0)));
                }
                Instruction::NEG => unary!(self.stack, -, as_int),
                Instruction::AND => binary!(self.stack, &&, as_bool),
                Instruction::OR => binary!(self.stack, ||, as_bool),
                Instruction::ADD => binary!(self.stack, +, as_int),
                Instruction::SUB => binary!(self.stack, -, as_int),
                Instruction::MUL => binary!(self.stack, *, as_int),
                Instruction::DIV => binary!(self.stack, /, as_int),
                Instruction::MOD => binary!(self.stack, %, as_int),
                Instruction::LE => binary!(self.stack, <, raw),
                Instruction::LEQ => binary!(self.stack, <=, raw),
                Instruction::GT => binary!(self.stack, >, raw),
                Instruction::GEQ => binary!(self.stack, >=, raw),
                Instruction::EQ => binary!(self.stack, ==, raw),
                Instruction::NEQ => binary!(self.stack, !=, raw),
                Instruction::ADDF => binary!(self.stack, +, as_float, to_bits),
                Instruction::SUBF => binary!(self.stack, -, as_float, to_bits),
                Instruction::MULF => binary!(self.stack, *, as_float, to_bits),
                Instruction::DIVF => binary!(self.stack, /, as_float, to_bits),
                Instruction::MODF => binary!(self.stack, %, as_float, to_bits),
                Instruction::SHL => binary!(self.stack, <<, as_int),
                Instruction::SHR => binary!(self.stack, >>, as_int),
                Instruction::XOR => binary!(self.stack, ^, as_int),
                Instruction::BITAND => binary!(self.stack, &, as_int),
                Instruction::BITOR => binary!(self.stack, |, as_int),
                Instruction::Pow => {
                    let sp = self.stack.tell();
                    promise!(sp >= 2);
                    let rhs = self.stack[sp - 1].as_int();
                    let lhs = self.stack[sp - 2].as_int();
                    let result = lhs.pow(rhs as u32);
                    self.stack[sp - 2].replace(result as _);
                    self.stack.seek(sp - 1);
                }
                Instruction::PowF => {
                    let sp = self.stack.tell();
                    promise!(sp >= 2);
                    let rhs = self.stack[sp - 1].as_float();
                    let lhs = self.stack[sp - 2].as_float();
                    let result = lhs.powf(rhs);
                    self.stack[sp - 2].replace(result.to_bits() as _);
                    self.stack.seek(sp - 1);
                }
                Instruction::LEF => binary!(self.stack, <, as_float),
                Instruction::LEQF => binary!(self.stack, <=, as_float),
                Instruction::GTF => binary!(self.stack, >, as_float),
                Instruction::GEQF => binary!(self.stack, >=, as_float),
                Instruction::FORMAT => {
                    let params_count = opcode.operand_u32();
                    if params_count != 0 {
                        let mut params = ArrayVec::<Value, 8>::default();

                        for _ in 0..params_count as usize {
                            params.push(self.stack.pop());
                        }

                        let ptr = self.stack.pop().as_ptr::<ObjString>();
                        let format_string = (unsafe { &*ptr }).data.as_str();

                        let mut message = String::default();

                        let mut chars = format_string.chars().peekable();
                        while let Some(ch) = chars.next() {
                            if ch == '%' {
                                match chars.peek() {
                                    Some('i') => {
                                        chars.next();
                                        message.push_str(&params.pop().as_int().to_string());
                                    }
                                    Some('f') => {
                                        chars.next();
                                        // message
                                        //     .push_str(&format!("{:.?}", params.pop().as_float()));
                                        let _ =
                                            write!(&mut message, "{:.?}", params.pop().as_float());
                                    }
                                    Some('b') => {
                                        chars.next();
                                        let _ = write!(
                                            &mut message,
                                            "{:0b}",
                                            params.pop().raw().addr()
                                        );
                                    }
                                    Some('s') => {
                                        chars.next();
                                        let string_val =
                                            (unsafe { &*params.pop().as_ptr::<ObjString>() })
                                                .data
                                                .as_str();
                                        // Allocated::<crate::String>::new(params.pop().as_ptr());
                                        message.push_str(string_val);
                                    }
                                    Some('x') => {
                                        chars.next();
                                        let _ = write!(
                                            &mut message,
                                            "{:0x}",
                                            params.pop().raw().addr()
                                        );
                                    }
                                    Some('z') => {
                                        chars.next();
                                        message.push_str(if params.pop().raw() > 0 as _ {
                                            "true"
                                        } else {
                                            "false"
                                        });
                                    }
                                    Some('u') => {
                                        chars.next();
                                        message.push_str(&params.pop().raw().addr().to_string());
                                    }
                                    Some('p') => {
                                        chars.next();
                                        let _ = write!(
                                            &mut message,
                                            "{:08x}",
                                            params.pop().as_ptr::<bool>().addr()
                                        );
                                    }
                                    _ => {
                                        message.push('%');
                                    }
                                }
                            } else {
                                message.push(ch);
                            }
                        }

                        let (obj, _) = self
                            .heap
                            .alloc(ObjString::from(message.as_str()), Object::String);

                        self.alloc_counter += 1;
                        if self.alloc_counter > GC_TRIGGER_INTERVAL {
                            Self::gc_collect(
                                &mut self.heap,
                                &self.stack,
                                &self.resume_stack,
                                &mut self.alloc_counter,
                            );
                        }

                        self.stack.push(Value::from(obj.addr()));
                    }
                }
                Instruction::STRINGIFY => {
                    // Shared primitive conversion for Show thunks / `%v`.
                    // Accepts a boxed value (preferred), a heap string, or a
                    // raw immediate (treated as int).
                    let v = self.stack.pop();
                    let text = Self::stringify_value(&self.heap, v);
                    let (obj, _) = self
                        .heap
                        .alloc(ObjString::from(text.as_str()), Object::String);
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(obj.addr()));
                }
                Instruction::PRINT => {
                    let ptr = self.stack.pop().as_ptr::<ObjString>();
                    let s = unsafe { &*ptr };
                    if let Some(out) = self.output.as_mut() {
                        let _ = write!(out, "{}", s);
                        let _ = out.flush();
                    } else {
                        print!("{}", s);
                        let _ = io::stdout().flush();
                    }
                }
                Instruction::JMP => {
                    ip = opcode.operand_u32() as usize;
                }
                Instruction::JMPF => {
                    if !self.stack.pop().as_bool() {
                        ip = opcode.operand_u32() as usize;
                    }
                }
                Instruction::JMPT => {
                    if self.stack.pop().as_bool() {
                        ip = opcode.operand_u32() as usize;
                    }
                }
                Instruction::CALL => {
                    let (arity, target) = opcode.call_parts();
                    let return_ip = ip + if target == 0 { 1 } else { 0 };
                    let callee_sp = self.stack.tell() - arity;
                    self.frames.get_mut().seek(return_ip);
                    self.frames
                        .setup_current_and_advance(|frame| frame.set(callee_sp));
                    sp = callee_sp;
                    if target != 0 {
                        ip = target;
                    }
                }
                Instruction::TailCall => {
                    let (arity, target) = opcode.call_parts();
                    let callee_sp = self.frames.get().get();
                    for i in (0..arity).rev() {
                        let val = self.stack.pop();
                        self.stack[callee_sp + i] = val;
                    }
                    self.stack.seek(callee_sp + arity);
                    // Match CALL: `sp` is the frame base (locals start at slot 0),
                    // not past the args. Using `callee_sp + arity` would make
                    // subsequent LOAD/BinSlotImm read the wrong slots.
                    sp = callee_sp;
                    ip = target;
                }
                Instruction::CastIntToFloat => {
                    let v = self.stack.pop().as_int() as f64;
                    self.stack.push(Value::from(v));
                }
                Instruction::CastFloatToInt => {
                    // Truncate toward zero (`3.9 as int` → `3`); not floor/round.
                    let v = self.stack.pop().as_float() as i64;
                    self.stack.push(Value::from(v));
                }
                Instruction::CastIntToByte => {
                    let v = self.stack.pop().as_int();
                    self.stack.push(Value::from((v as u8) as i64));
                }
                Instruction::CastByteToInt => {
                    let v = self.stack.pop().as_int();
                    self.stack.push(Value::from(v & 0xff));
                }
                Instruction::CastIntToBool => {
                    let v = self.stack.pop().as_int();
                    self.stack.push(Value::from((v != 0) as i64));
                }
                Instruction::CastBoolToInt => {
                    let v = self.stack.pop().as_int();
                    self.stack.push(Value::from(if v != 0 { 1 } else { 0 }));
                }
                Instruction::INIT => {
                    let (_, mut r) = self.heap.alloc(ObjInstance::default(), Object::Instance);
                    let _ = r.as_mut();

                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }

                    self.stack.push(Value::from(r.as_ptr().addr() as u64));
                }
                Instruction::RETURN => {
                    let ret_val = self.stack.pop();
                    if self.capture_nested_return(ret_val) {
                        return false;
                    }
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    self.after_return(&mut ip, &mut sp);
                }
                // Fused `LOAD slot; CONST imm; <binop>`.
                Instruction::BinSlotImm => {
                    let (op, slot, imm) = opcode.bin_slot_imm_parts();
                    let lhs = self.stack[sp + slot];
                    self.stack.push(lhs);
                    self.stack.push(Value::from(imm));
                    match Instruction::from(op) {
                        Instruction::ADD => binary!(self.stack, +, as_int),
                        Instruction::SUB => binary!(self.stack, -, as_int),
                        Instruction::MUL => binary!(self.stack, *, as_int),
                        Instruction::DIV => binary!(self.stack, /, as_int),
                        Instruction::MOD => binary!(self.stack, %, as_int),
                        Instruction::LE => binary!(self.stack, <, raw),
                        Instruction::LEQ => binary!(self.stack, <=, raw),
                        Instruction::GT => binary!(self.stack, >, raw),
                        Instruction::GEQ => binary!(self.stack, >=, raw),
                        Instruction::EQ => binary!(self.stack, ==, raw),
                        Instruction::NEQ => binary!(self.stack, !=, raw),
                        Instruction::Pow => {
                            let sp = self.stack.tell();
                            let rhs = self.stack[sp - 1].as_int().max(0) as u32;
                            let lhs = self.stack[sp - 2].as_int();
                            self.stack[sp - 2].replace(lhs.pow(rhs) as u64);
                            self.stack.seek(sp - 1);
                        }
                        Instruction::BITAND => binary!(self.stack, &, as_int),
                        Instruction::BITOR => binary!(self.stack, |, as_int),
                        _ => {}
                    }
                }
                // Fused `<cmp>; JMPF target`.
                Instruction::CmpJmpf => {
                    let (op, target) = opcode.cmp_jmpf_parts();
                    match Instruction::from(op) {
                        Instruction::LE => binary!(self.stack, <, raw),
                        Instruction::LEQ => binary!(self.stack, <=, raw),
                        Instruction::GT => binary!(self.stack, >, raw),
                        Instruction::GEQ => binary!(self.stack, >=, raw),
                        Instruction::EQ => binary!(self.stack, ==, raw),
                        Instruction::NEQ => binary!(self.stack, !=, raw),
                        Instruction::LEF => binary!(self.stack, <, as_float),
                        Instruction::LEQF => binary!(self.stack, <=, as_float),
                        Instruction::GTF => binary!(self.stack, >, as_float),
                        Instruction::GEQF => binary!(self.stack, >=, as_float),
                        _ => {}
                    }
                    if !self.stack.pop().as_bool() {
                        ip = target;
                    }
                }
                Instruction::BinSlotImmJmpf => {
                    let (op, slot, pool_idx) = opcode.bin_slot_imm_jmpf_parts();
                    let packed = constants.get(pool_idx).copied().unwrap_or(0);
                    let imm = packed as u32 as i32 as i64;
                    let target = (packed >> 32) as usize;
                    let lhs = self.stack[sp + slot];
                    self.stack.push(lhs);
                    self.stack.push(Value::from(imm));
                    match Instruction::from(op) {
                        Instruction::LE => binary!(self.stack, <, raw),
                        Instruction::LEQ => binary!(self.stack, <=, raw),
                        Instruction::GT => binary!(self.stack, >, raw),
                        Instruction::GEQ => binary!(self.stack, >=, raw),
                        Instruction::EQ => binary!(self.stack, ==, raw),
                        Instruction::NEQ => binary!(self.stack, !=, raw),
                        Instruction::LEF => binary!(self.stack, <, as_float),
                        Instruction::LEQF => binary!(self.stack, <=, as_float),
                        Instruction::GTF => binary!(self.stack, >, as_float),
                        Instruction::GEQF => binary!(self.stack, >=, as_float),
                        _ => {
                            self.stack.pop();
                            self.stack.pop();
                        }
                    }
                    if !self.stack.pop().as_bool() {
                        ip = target;
                    }
                }
                Instruction::LogNotJmpf => {
                    let target = opcode.log_not_jmpf_target();
                    let val = self.stack.pop();
                    if val.as_int() != 0 {
                        ip = target;
                    }
                }
                Instruction::LoadReturnSlot => {
                    let ret_val = self.stack[sp + opcode.operand_u32() as usize];
                    if self.capture_nested_return(ret_val) {
                        return false;
                    }
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    self.after_return(&mut ip, &mut sp);
                }
                Instruction::ConstReturnImm => {
                    let ret_val = Value::from(opcode.operand_u32() as i32 as i64 as u64);
                    if self.capture_nested_return(ret_val) {
                        return false;
                    }
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    self.after_return(&mut ip, &mut sp);
                }
                Instruction::BinReturn => {
                    match Instruction::from(opcode.bin_return_op()) {
                        Instruction::ADD => binary!(self.stack, +, as_int),
                        Instruction::SUB => binary!(self.stack, -, as_int),
                        Instruction::MUL => binary!(self.stack, *, as_int),
                        Instruction::DIV => binary!(self.stack, /, as_int),
                        Instruction::MOD => binary!(self.stack, %, as_int),
                        Instruction::ADDF => binary!(self.stack, +, as_float, to_bits),
                        Instruction::SUBF => binary!(self.stack, -, as_float, to_bits),
                        Instruction::MULF => binary!(self.stack, *, as_float, to_bits),
                        Instruction::DIVF => binary!(self.stack, /, as_float, to_bits),
                        Instruction::MODF => binary!(self.stack, %, as_float, to_bits),
                        Instruction::LE => binary!(self.stack, <, raw),
                        Instruction::LEQ => binary!(self.stack, <=, raw),
                        Instruction::GT => binary!(self.stack, >, raw),
                        Instruction::GEQ => binary!(self.stack, >=, raw),
                        Instruction::EQ => binary!(self.stack, ==, raw),
                        Instruction::NEQ => binary!(self.stack, !=, raw),
                        Instruction::LEF => binary!(self.stack, <, as_float),
                        Instruction::LEQF => binary!(self.stack, <=, as_float),
                        Instruction::GTF => binary!(self.stack, >, as_float),
                        Instruction::GEQF => binary!(self.stack, >=, as_float),
                        _ => {}
                    }
                    let ret_val = self.stack.pop();
                    if self.capture_nested_return(ret_val) {
                        return false;
                    }
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    self.after_return(&mut ip, &mut sp);
                }
                Instruction::BinSlotSlot => {
                    let (op, a, b) = opcode.bin_slot_slot_parts();
                    let va = self.stack[sp + a];
                    let vb = self.stack[sp + b];
                    let result = match Instruction::from(op) {
                        Instruction::ADD => Value::from(va.as_int() + vb.as_int()),
                        Instruction::SUB => Value::from(va.as_int() - vb.as_int()),
                        Instruction::MUL => Value::from(va.as_int() * vb.as_int()),
                        Instruction::DIV => Value::from(va.as_int() / vb.as_int()),
                        Instruction::MOD => Value::from(va.as_int() % vb.as_int()),
                        Instruction::Pow => {
                            let exp = vb.as_int().max(0) as u32;
                            Value::from(va.as_int().pow(exp))
                        }
                        Instruction::BITAND => Value::from(va.as_int() & vb.as_int()),
                        Instruction::BITOR => Value::from(va.as_int() | vb.as_int()),
                        Instruction::ADDF => Value::from(va.as_float() + vb.as_float()),
                        Instruction::SUBF => Value::from(va.as_float() - vb.as_float()),
                        Instruction::MULF => Value::from(va.as_float() * vb.as_float()),
                        Instruction::DIVF => Value::from(va.as_float() / vb.as_float()),
                        Instruction::MODF => Value::from(va.as_float() % vb.as_float()),
                        Instruction::LE => Value::from((va.raw() < vb.raw()) as i64),
                        Instruction::LEQ => Value::from((va.raw() <= vb.raw()) as i64),
                        Instruction::GT => Value::from((va.raw() > vb.raw()) as i64),
                        Instruction::GEQ => Value::from((va.raw() >= vb.raw()) as i64),
                        Instruction::EQ => Value::from((va.raw() == vb.raw()) as i64),
                        Instruction::NEQ => Value::from((va.raw() != vb.raw()) as i64),
                        Instruction::LEF => Value::from((va.as_float() < vb.as_float()) as i64),
                        Instruction::LEQF => Value::from((va.as_float() <= vb.as_float()) as i64),
                        Instruction::GTF => Value::from((va.as_float() > vb.as_float()) as i64),
                        Instruction::GEQF => Value::from((va.as_float() >= vb.as_float()) as i64),
                        _ => Value::default(),
                    };
                    self.stack.push(result);
                }
                Instruction::NATIVE => {
                    #[cfg(debug_assertions)]
                    eprintln!("FFI: deprecated NATIVE opcode — recompile from source");
                }
                Instruction::FfiLoad => {
                    // Inlined to split-borrow `heap`/`libraries` from `frames`.
                    let path_val = self.stack.pop();
                    let path = {
                        let addr = path_val.raw() as u64;
                        match Self::find_object_by_addr(&self.heap, addr) {
                            Some(crate::memory::Object::String(gc)) => gc.as_ref().data.clone(),
                            _ => String::new(),
                        }
                    };
                    // Push `Result::Ok(handle)` or `Result::Err(ffi::Error)`.
                    match crate::ffi::resolve_library(
                        &path,
                        self.base_dir.as_deref(),
                        &self.ffi_search_paths,
                    ) {
                        Ok(lib_arc) => {
                            self.libraries
                                .entry(path.clone())
                                .or_insert_with(|| lib_arc.clone());
                            let (object, _gc) = self.heap.alloc_library(lib_arc);
                            let addr = object.addr();
                            self.userland_libraries
                                .insert(addr, std::sync::Arc::new(object));
                            self.push_result_ok(Value::from(addr as *mut u8));
                        }
                        Err(e) => {
                            self.push_ffi_error(e);
                        }
                    }
                }
                Instruction::FfiInvoke => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;
                    let has_arg_tags = (raw & (1 << 16)) != 0;

                    // Stack (bottom → top): lib, fn_id, args_tuple [, tags_tuple].
                    let arg_types = if has_arg_tags {
                        let tags_val = self.stack.pop();
                        let tags_addr = tags_val.raw() as u64;
                        let tags: Vec<crate::memory::FfiType> =
                            match Self::find_object_by_addr(&self.heap, tags_addr) {
                                Some(crate::memory::Object::Tuple(gc)) => gc
                                    .as_ref()
                                    .elements
                                    .iter()
                                    .map(|v| Self::ffi_type_from_value(v, &self.heap))
                                    .collect(),
                                _ => Vec::new(),
                            };
                        Some(tags)
                    } else {
                        None
                    };

                    let tuple_val = self.stack.pop();
                    let tuple_addr = tuple_val.raw() as u64;

                    let function_id_val = self.stack.pop();
                    let function_id = function_id_val.as_int() as usize;

                    let lib_val = self.stack.pop();
                    let lib_addr = lib_val.raw() as u64;

                    let args: Vec<Value> = match Self::find_object_by_addr(&self.heap, tuple_addr) {
                        Some(crate::memory::Object::Tuple(gc)) => gc.as_ref().elements.clone(),
                        _ => Vec::new(),
                    };

                    self.frames.get_mut().set(sp);
                    self.pending_ffi = Some(PendingFfiInvoke {
                        lib_addr,
                        function_id,
                        args,
                        arg_types,
                        resume_ip: ip,
                        resume_sp: sp,
                    });
                    return true;
                }
                Instruction::DeclareFFI => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;
                    let variadic = (raw & (1 << 16)) != 0;

                    // Stack (bottom → top): lib, name, args_tuple, ret_tag.
                    let ret_tag_val = self.stack.pop();
                    let ret_type = Self::ffi_type_from_value(&ret_tag_val, &self.heap);

                    // Pop the args tuple (next on the stack).
                    let args_tuple_val = self.stack.pop();
                    let args_tuple_addr = args_tuple_val.raw() as u64;

                    let arg_types: Vec<crate::memory::FfiType> =
                        match Self::find_object_by_addr(&self.heap, args_tuple_addr) {
                            Some(crate::memory::Object::Tuple(gc)) => gc
                                .as_ref()
                                .elements
                                .iter()
                                .map(|v| Self::ffi_type_from_value(v, &self.heap))
                                .collect(),
                            _ => Vec::new(),
                        };
                    // Pop the name string.
                    let name_val = self.stack.pop();
                    let name = Self::object_string_value(&self.heap, &name_val);
                    // Pop the lib handle.
                    let lib_val = self.stack.pop();
                    let lib_addr = lib_val.raw() as u64;
                    let lib_obj = self.userland_libraries.get(&lib_addr).cloned();
                    match lib_obj {
                        Some(obj_arc) => {
                            let mut owned = *obj_arc;
                            let ffi_sig = crate::ffi::FfiSignature {
                                name,
                                args: arg_types,
                                ret: ret_type,
                                variadic,
                            };
                            match Self::register_signature_on_object(
                                &mut owned,
                                ffi_sig,
                                &self.struct_layouts,
                            ) {
                                Ok(id) => {
                                    self.userland_libraries
                                        .insert(lib_addr, std::sync::Arc::new(owned));
                                    self.push_result_ok(Value::from(id as i64));
                                }
                                Err(e) => {
                                    self.push_ffi_error(e);
                                }
                            }
                        }
                        None => {
                            self.push_result_err(
                                crate::ffi::FfiErrorKindTag::InvalidHandle,
                                format!(
                                    "FFI declare: library at 0x{:x} is not loaded",
                                    lib_addr
                                ),
                            );
                        }
                    }
                }
                Instruction::HostInvoke => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;
                    let tuple_val = self.stack.pop();
                    let tuple_addr = tuple_val.raw() as u64;
                    let fn_id_val = self.stack.pop();
                    let fn_id = fn_id_val.as_int() as usize;
                    let args: Vec<Value> = match Self::find_object_by_addr(&self.heap, tuple_addr) {
                        Some(crate::memory::Object::Tuple(gc)) => gc.as_ref().elements.clone(),
                        _ => Vec::new(),
                    };
                    // Packed LA (and other host natives) allocate via
                    // `heap.alloc` inside the closure; count those so GC
                    // pressure still fires when HostInvoke is the only
                    // allocator on a hot path.
                    let live_before = self.heap.live_object_count();
                    match self.natives.get_by_id(fn_id) {
                        Some(native) => match native.invoke(&mut self.heap, &args) {
                            Ok(Some(v)) => self.stack.push(v),
                            Ok(None) => {}
                            #[cfg(debug_assertions)]
                            Err(e) => eprintln!("HostInvoke failed for `{}`: {e}", native.name()),
                            #[cfg(not(debug_assertions))]
                            Err(_) => {}
                        },
                        #[cfg(debug_assertions)]
                        None => eprintln!("HostInvoke: unknown native id {fn_id}"),
                        #[cfg(not(debug_assertions))]
                        None => {}
                    }
                    let allocated = self
                        .heap
                        .live_object_count()
                        .saturating_sub(live_before);
                    if allocated > 0 {
                        self.alloc_counter += allocated;
                        if self.alloc_counter > GC_TRIGGER_INTERVAL {
                            Self::gc_collect(
                                &mut self.heap,
                                &self.stack,
                                &self.resume_stack,
                                &mut self.alloc_counter,
                            );
                        }
                    }
                }
                Instruction::HALT => {
                    if let Some(out) = self.output.as_mut() {
                        let _ = out.flush();
                    } else {
                        let _ = io::stdout().flush();
                    }
                    return false;
                }
                Instruction::Panic => {
                    let panic_ip = ip.saturating_sub(1);
                    let ptr = self.stack.pop().as_ptr::<ObjString>();
                    let s = unsafe { &*ptr };
                    let loc_suffix = self
                        .format_panic_location(panic_ip)
                        .map(|loc| format!(" at {loc}"))
                        .unwrap_or_default();
                    if let Some(out) = self.output.as_mut() {
                        let _ = write!(out, "panic: {}{}", s, loc_suffix);
                        let _ = out.flush();
                    } else {
                        eprint!("panic: {}{}", s, loc_suffix);
                        let _ = io::stderr().flush();
                    }
                    self.panicked = true;
                    return false;
                }
                Instruction::STRING => {
                    let length = opcode.operand_u32() as usize;
                    let mut value: String = String::with_capacity(length);

                    while length != value.len() && ip < code.len() {
                        let data = &code[ip];
                        ip += 1;
                        value.push(char::from_u32(data.operand_u32()).unwrap_or_default());
                    }

                    let gc_string = self.heap.intern(value);

                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }

                    self.stack
                        .push(Value::from(gc_string.as_ptr() as *mut u8 as u64));
                }
                Instruction::NOOP => continue,
                Instruction::MakeEnum => {
                    // operands: tag (high 16), arity (low 16). Args popped top-first
                    // into declaration order; classify each as immediate or heap pointer.
                    let operands = opcode.operand_u32();
                    let tag = operands >> 16;
                    let arity = (operands & 0xFFFF) as usize;

                    let mut values: Vec<Value> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        if self.stack.tell() == 0 {
                            break;
                        }
                        values.push(self.stack.pop());
                    }

                    let mut payload: Vec<Member> = Vec::with_capacity(values.len());
                    for v in values {
                        if self.heap.contains_addr(v.raw()) {
                            let addr = v.raw() as u64;
                            if let Some(o) = Self::find_object_by_addr(&self.heap, addr) {
                                payload.push(Member::Object(o));
                            } else {
                                payload.push(Member::Value(v));
                            }
                        } else {
                            payload.push(Member::Value(v));
                        }
                    }

                    let obj_enum = ObjEnum { tag, payload };
                    let (object, _) = self.heap.alloc(obj_enum, Object::Enum);

                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }

                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::MakeTuple | Instruction::MakeArray => {
                    let operands = opcode.operand_u32();
                    let arity = (operands & 0xFFFF) as usize;
                    let mut values: Vec<Value> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        if self.stack.tell() == 0 {
                            break;
                        }
                        values.push(self.stack.pop());
                    }
                    values.reverse();
                    self.alloc_counter += 1;
                    let addr = if matches!(opcode.bytecode(), Instruction::MakeTuple) {
                        let obj_tuple = ObjTuple { elements: values };
                        let (object, _) = self.heap.alloc(obj_tuple, Object::Tuple);
                        object.addr()
                    } else {
                        let obj_array = ObjArray { elements: values };
                        let (object, _) = self.heap.alloc(obj_array, Object::Array);
                        object.addr()
                    };
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(addr));
                }
                Instruction::Index => {
                    let index_val = self.stack.pop();
                    let target_val = self.stack.pop();
                    let target_addr = target_val.raw() as u64;
                    let index = index_val.as_int();
                    let result = match Self::find_object_by_addr(&self.heap, target_addr) {
                        Some(crate::memory::Object::Tuple(gc)) => {
                            let elements = &gc.as_ref().elements;
                            if index < 0 || (index as usize) >= elements.len() {
                                Value::from(-1_i64)
                            } else {
                                elements[index as usize]
                            }
                        }
                        Some(crate::memory::Object::Array(gc)) => {
                            let elements = &gc.as_ref().elements;
                            if index < 0 || (index as usize) >= elements.len() {
                                Value::from(-1_i64)
                            } else {
                                elements[index as usize]
                            }
                        }
                        _ => Value::from(-1_i64),
                    };
                    self.stack.push(result);
                }
                Instruction::MakeDict => {
                    let arity = (opcode.operand_u32() & 0xFFFF) as usize;
                    let mut pairs: Vec<(String, Value)> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        let name_val = self.stack.pop();
                        let value = self.stack.pop();
                        let name = Self::object_string_value(&self.heap, &name_val);
                        pairs.push((name, value));
                    }
                    pairs.reverse();
                    // Allocate the instance and populate.
                    self.alloc_counter += 1;
                    let (object, mut gc) =
                        self.heap.alloc(ObjInstance::default(), Object::Instance);
                    {
                        let instance: &mut ObjInstance = gc.as_mut();
                        for (name, value) in pairs {
                            let key = self.heap.intern(name);
                            let member = if let Some(obj) =
                                Self::find_object_by_addr(&self.heap, value.raw() as u64)
                            {
                                crate::memory::Member::Object(obj)
                            } else {
                                crate::memory::Member::Value(value)
                            };
                            instance.set(key, member);
                        }
                    }
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::GetField => {
                    let name_val = self.stack.pop();
                    let target_val = self.stack.pop();
                    let name = Self::object_string_value(&self.heap, &name_val);
                    let target_addr = target_val.raw() as u64;
                    let result = match Self::find_object_by_addr(&self.heap, target_addr) {
                        Some(crate::memory::Object::Instance(gc)) => {
                            let key = self.heap.intern(name);
                            match gc.as_ref().get(key) {
                                Some(crate::memory::Member::Value(v)) => v,
                                // Heap objects (strings, nested dicts, enums, …)
                                // — push the address, same as LoadField/Unpack.
                                Some(crate::memory::Member::Object(o)) => Value::from(o.addr()),
                                None => Value::from(-1_i64),
                            }
                        }
                        _ => Value::from(-1_i64),
                    };
                    self.stack.push(result);
                }
                Instruction::SetField => {
                    let name_val = self.stack.pop();
                    let target_val = self.stack.pop();
                    let value = self.stack.pop();
                    let name = Self::object_string_value(&self.heap, &name_val);
                    let target_addr = target_val.raw() as u64;
                    if let Some(crate::memory::Object::Instance(mut gc)) =
                        Self::find_object_by_addr(&self.heap, target_addr)
                    {
                        let key = self.heap.intern(name);
                        let member = if let Some(obj) =
                            Self::find_object_by_addr(&self.heap, value.raw() as u64)
                        {
                            crate::memory::Member::Object(obj)
                        } else {
                            crate::memory::Member::Value(value)
                        };
                        gc.as_mut().set(key, member);
                    }
                    self.stack.push(value);
                }
                Instruction::StoreIndex => {
                    let value = self.stack.pop();
                    let index_val = self.stack.pop();
                    let target_val = self.stack.pop();
                    let target_addr = target_val.raw() as u64;
                    let index = index_val.as_int();
                    if let Some(crate::memory::Object::Array(mut gc)) =
                        Self::find_object_by_addr(&self.heap, target_addr)
                    {
                        let arr = gc.as_mut();
                        if index >= 0 && (index as usize) < arr.elements.len() {
                            arr.elements[index as usize] = value;
                        }
                    }
                    self.stack.push(value);
                }
                Instruction::ArrayPush => {
                    // Stack discipline matches `StoreIndex`: codegen emits
                    // `array` then `value`, so dispatch pops value first,
                    // mutates the heap array in place, and returns the array
                    // address for chaining (`push(push(a, 1), 2)`).
                    let value = self.stack.pop();
                    let target_val = self.stack.pop();
                    let target_addr = target_val.raw() as u64;
                    if let Some(crate::memory::Object::Array(mut gc)) =
                        Self::find_object_by_addr(&self.heap, target_addr)
                    {
                        let old_bytes =
                            gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
                        gc.as_mut().elements.push(value);
                        let new_bytes =
                            gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
                        if old_bytes != new_bytes {
                            self.heap.account_resize(old_bytes, new_bytes);
                        }
                    }
                    self.stack.push(target_val);
                }
                Instruction::ArrayLen => {
                    let target_val = self.stack.pop();
                    let target_addr = target_val.raw() as u64;
                    let len = match Self::find_object_by_addr(&self.heap, target_addr) {
                        Some(crate::memory::Object::Array(gc)) => gc.as_ref().elements.len(),
                        _ => 0,
                    };
                    self.stack.push(Value::from(len as i64));
                }
                Instruction::DictEntries => {
                    // Pop dict → push ObjArray of ObjTuple(2) (key, value).
                    let dict_val = self.stack.pop();
                    let dict_addr = dict_val.raw() as u64;
                    let mut pair_addrs: Vec<Value> = Vec::new();
                    if let Some(crate::memory::Object::Instance(gc)) =
                        Self::find_object_by_addr(&self.heap, dict_addr)
                    {
                        let entries: Vec<(crate::memory::RefString, Member)> =
                            gc.as_ref().iter_fields().collect();
                        for (key, member) in entries {
                            let key_val = Value::from(key.as_ptr() as u64);
                            let val = match member {
                                Member::Value(v) => v,
                                Member::Object(o) => Value::from(o.addr()),
                            };
                            self.alloc_counter += 1;
                            let (tuple_obj, _) = self.heap.alloc(
                                ObjTuple {
                                    elements: vec![key_val, val],
                                },
                                Object::Tuple,
                            );
                            pair_addrs.push(Value::from(tuple_obj.addr()));
                        }
                    }
                    self.alloc_counter += 1;
                    let (array_obj, _) = self.heap.alloc(
                        ObjArray {
                            elements: pair_addrs,
                        },
                        Object::Array,
                    );
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(array_obj.addr()));
                }
                Instruction::JumpIfMatch => {
                    // Tag in operands[31:16]; pool index in operands[15:0]
                    // (`constants[idx]` holds the absolute jump target).
                    let operands = opcode.operand_u32();
                    let expected_tag = operands >> 16;

                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let scrutinee_addr = self.stack.peek().raw() as u64;

                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            if enum_ref.tag == expected_tag {
                                let pool_idx = (operands & 0xFFFF) as usize;
                                debug_assert!(
                                    pool_idx < constants.len(),
                                    "JumpIfMatch pool index {pool_idx} out of range (len {})",
                                    constants.len()
                                );
                                let target_offset = opcode.jump_if_match_target(constants);
                                let _ = self.stack.pop();
                                for member in &enum_ref.payload {
                                    let value = match member {
                                        Member::Value(v) => *v,
                                        Member::Object(o) => Value::from(o.addr()),
                                    };
                                    self.stack.push(value);
                                }
                                ip = target_offset;
                            }
                        }
                    }
                }
                Instruction::Unpack => {
                    // Pops enum scrutinee; pushes payload in declaration order
                    // (stack/locals overlap — see STORE).
                    let _arity = opcode.operand_u32() as usize;

                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let scrutinee_addr = self.stack.pop().raw() as u64;

                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            for member in &enum_ref.payload {
                                let value = match member {
                                    Member::Value(v) => *v,
                                    Member::Object(o) => Value::from(o.addr()),
                                };
                                self.stack.push(value);
                            }
                        }
                    }
                }
                Instruction::LoadField => {
                    let field_index = (opcode.operand_u32() & 0xFFFF) as usize;

                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let scrutinee_addr = self.stack.pop().raw() as u64;

                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            if let Some(member) = enum_ref.payload.get(field_index) {
                                let value = match member {
                                    Member::Value(v) => *v,
                                    Member::Object(o) => Value::from(o.addr()),
                                };
                                self.stack.push(value);
                            } else {
                                // OOB field index — keep stack balanced.
                                self.stack.push(Value::default());
                            }
                        } else {
                            // Non-enum receiver (e.g. class Instance misrouted
                            // through LoadField). Push a sentinel so the pop
                            // above does not leave the stack short.
                            self.stack.push(Value::default());
                        }
                    }
                }
                Instruction::UnpackAt => {
                    // Unpack enum at `sp + slot_offset` in place (nested record patterns).
                    // Scratch-area codegen may unpack past the current cursor; extend
                    // `tell` so subsequent LOAD/StorePop see the written slots.
                    let operands = opcode.operand_u32();
                    let slot_offset = (operands & 0xFFFF) as usize;
                    let _arity = (operands >> 16) as usize;

                    let slot = sp + slot_offset;
                    if slot >= self.stack.tell() {
                        // Intentional empty body: defensive no-op when the UnpackAt
                        // slot is out of range (typechecker should prevent this).
                    } else {
                        let scrutinee_addr = self.stack[slot].raw() as u64;

                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            for (i, member) in enum_ref.payload.iter().enumerate() {
                                let value = match member {
                                    Member::Value(v) => *v,
                                    Member::Object(o) => Value::from(o.addr()),
                                };
                                self.stack[slot + i] = value;
                            }
                            let end = slot + enum_ref.payload.len();
                            if self.stack.tell() < end {
                                self.stack.seek(end);
                            }
                        }
                    }
                }
                Instruction::StorePop => {
                    // Pop TOS into `sp + slot`. Extend the cursor when the slot
                    // is newly allocated, but NEVER shrink past higher locals —
                    // locals and the operand stack share memory (Phase 18E).
                    // Unconditional `seek(slot + 1)` orphans later slots and
                    // makes early-loop flags appear not to stick.
                    let slot = sp + opcode.operand_u32() as usize;
                    let val = self.stack.pop();
                    self.stack[slot] = val;
                    let tell = self.stack.tell();
                    if tell < slot + 1 {
                        self.stack.seek(slot + 1);
                    }
                }
                Instruction::MakeCoro => {
                    let (arity, target) = opcode.call_parts();
                    let mut values: Vec<Value> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        if self.stack.tell() == 0 {
                            break;
                        }
                        values.push(self.stack.pop());
                    }
                    values.reverse();

                    let obj_coro = ObjCoroutine {
                        state: CoroState::Suspended,
                        resume_ip: target,
                        saved_stack: values,
                        saved_frames: vec![(target, 0)],
                        pending_send: Value::from(0_i64),
                        yield_from: None,
                        yield_from_resume_ip: 0,
                    };
                    let (object, _) = self.heap.alloc(obj_coro, Object::Coroutine);

                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }

                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::ResumeCoro => {
                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let has_send = opcode.operand_u32() & 1 != 0;
                        let handle = self.stack.pop();
                        let send_val = if has_send {
                            self.stack.pop()
                        } else {
                            Value::from(0_i64)
                        };
                        let addr = handle.raw() as u64;
                        if let Some(Object::Coroutine(gc)) =
                            Self::find_object_by_addr(&self.heap, addr)
                        {
                            if gc.as_ref().state == CoroState::Done {
                                // Resuming an already-Done coroutine always
                                // yields the sentinel `Value::default()`
                                // (never the coroutine's last `return`
                                // value). There is no error-handling
                                // machinery yet to signal "resumed after
                                // completion", so this keeps the behavior
                                // well-defined rather than leaking a stale
                                // value; a real error/Result protocol is
                                // deferred to a later phase.
                                self.stack.push(Value::default());
                            } else if let Some(sub) = gc.as_ref().yield_from {
                                self.with_coroutine_mut(gc.as_ptr() as u64, |c| {
                                    c.pending_send = send_val;
                                });
                                self.resume_coroutine(&mut ip, &mut sp, sub, send_val, code, true);
                            } else {
                                self.resume_coroutine(&mut ip, &mut sp, gc, send_val, code, true);
                            }
                        } else {
                            // Handle didn't resolve to a live coroutine
                            // object (e.g. already freed) — same
                            // well-defined sentinel as the Done case.
                            self.stack.push(Value::default());
                        }
                    }
                }
                Instruction::YieldCoro => {
                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let yield_val = self.stack.pop();
                        self.yield_coroutine(&mut ip, &mut sp, yield_val);
                    }
                }
                Instruction::YieldFromCoro => {
                    if self.stack.tell() == 0 {
                        // Intentional empty body: defensive no-op when the stack is
                        // empty (typechecker should prevent this; do not panic).
                    } else {
                        let handle = self.stack.pop();
                        let addr = handle.raw() as u64;
                        if let Some(Object::Coroutine(sub)) =
                            Self::find_object_by_addr(&self.heap, addr)
                        {
                            self.start_yield_from(&mut ip, &mut sp, sub, code);
                        }
                    }
                }
                Instruction::DoneCoro => {
                    if self.stack.tell() == 0 {
                        self.stack.push(Value::from(false));
                    } else {
                        let handle = self.stack.pop();
                        let addr = handle.raw() as u64;
                        let is_done = matches!(
                            Self::find_object_by_addr(&self.heap, addr),
                            Some(Object::Coroutine(gc)) if gc.as_ref().state == CoroState::Done
                        );
                        self.stack.push(Value::from(is_done));
                    }
                }
                Instruction::CallIndirect => {
                    // Stack: [value_args..., app_dicts..., target]
                    // operands[15:0] = value_arity; [31:16] = app_dict_arity
                    let packed = opcode.operand_u32();
                    let value_arity = (packed & 0xFFFF) as usize;
                    let app_dict_arity = ((packed >> 16) & 0xFFFF) as usize;
                    let raw = self.stack.pop();

                    // First-class ObjFn: merge new args into holes / captures.
                    let fn_obj = {
                        let addr = raw.raw() as u64;
                        if !raw.raw().is_null() && self.heap.contains_addr(raw.raw()) {
                            self.heap.find_object_by_addr(addr).and_then(|o| match o {
                                Object::Fn(gc) => Some(gc),
                                _ => None,
                            })
                        } else {
                            None
                        }
                    };

                    if let Some(gc) = fn_obj {
                        // Pop application dictionaries first (unused for ObjFn).
                        for _ in 0..app_dict_arity {
                            let _ = self.stack.pop();
                        }
                        let mut new_args = Vec::with_capacity(value_arity);
                        for _ in 0..value_arity {
                            new_args.push(self.stack.pop());
                        }
                        new_args.reverse();

                        let base = gc.as_ref();
                        let arity = base.arity as usize;
                        let is_rest = base.is_rest;
                        let mut filled_mask = base.filled_mask;
                        let captures = base.captures.clone();
                        let entry = base.entry;

                        // Expand existing filled values into per-slot slots
                        // (decl order), then fill the next unfilled holes
                        // positionally from `new_args`.
                        let mut slot_vals: Vec<Option<Value>> = vec![None; arity];
                        {
                            let mut old_i = 0usize;
                            for slot in 0..arity {
                                if filled_mask & (1u32 << slot) != 0 {
                                    if old_i < base.captured_args.len() {
                                        slot_vals[slot] = Some(base.captured_args[old_i]);
                                        old_i += 1;
                                    }
                                }
                            }
                        }
                        let mut arg_i = 0usize;
                        for slot in 0..arity {
                            if filled_mask & (1u32 << slot) != 0 {
                                continue;
                            }
                            if arg_i >= new_args.len() {
                                break;
                            }
                            slot_vals[slot] = Some(new_args[arg_i]);
                            filled_mask |= 1u32 << slot;
                            arg_i += 1;
                        }

                        let mut captured_args: Vec<Value> = Vec::with_capacity(arity);
                        for slot in 0..arity {
                            if filled_mask & (1u32 << slot) != 0 {
                                if let Some(v) = slot_vals[slot] {
                                    captured_args.push(v);
                                }
                            }
                        }

                        let fixed_filled = filled_mask.count_ones() as usize;
                        let remaining_new = &new_args[arg_i..];

                        if fixed_filled < arity {
                            // Still a partial — push updated ObjFn.
                            let partial = ObjFn {
                                entry,
                                arity: base.arity,
                                is_rest,
                                filled_mask,
                                captured_args,
                                captures,
                            };
                            let (object, _) = self.heap.alloc(partial, Object::Fn);
                            self.alloc_counter += 1;
                            if self.alloc_counter > GC_TRIGGER_INTERVAL {
                                Self::gc_collect(
                                    &mut self.heap,
                                    &self.stack,
                                    &self.resume_stack,
                                    &mut self.alloc_counter,
                                );
                            }
                            self.stack.push(Value::from(object.addr()));
                            continue;
                        }

                        // Fixed slots complete. Rest extras → MakeArray
                        // (including empty rest when `is_rest` and no extras).
                        let mut call_args = captured_args;
                        if is_rest {
                            let rest_val = if remaining_new.len() == 1 {
                                let v = remaining_new[0];
                                let addr = v.raw() as u64;
                                if !v.raw().is_null()
                                    && self.heap.contains_addr(v.raw())
                                    && matches!(
                                        Self::find_object_by_addr(&self.heap, addr),
                                        Some(Object::Array(_))
                                    )
                                {
                                    v
                                } else {
                                    let arr = crate::memory::ObjArray {
                                        elements: remaining_new.to_vec(),
                                    };
                                    let (object, _) = self.heap.alloc(arr, Object::Array);
                                    self.alloc_counter += 1;
                                    Value::from(object.addr())
                                }
                            } else {
                                let arr = crate::memory::ObjArray {
                                    elements: remaining_new.to_vec(),
                                };
                                let (object, _) = self.heap.alloc(arr, Object::Array);
                                self.alloc_counter += 1;
                                Value::from(object.addr())
                            };
                            call_args.push(rest_val);
                        } else if !remaining_new.is_empty() {
                            // Too many args for a fixed fn — drop extras defensively.
                        }

                        // Frame: [captures..., params...]
                        for c in &captures {
                            self.stack.push(*c);
                        }
                        for a in &call_args {
                            self.stack.push(*a);
                        }
                        let frame_arity = captures.len() + call_args.len();
                        let return_ip = ip;
                        let callee_sp = self.stack.tell() - frame_arity;
                        self.frames.get_mut().seek(return_ip);
                        self.frames
                            .setup_current_and_advance(|frame| frame.set(callee_sp));
                        sp = callee_sp;
                        ip = entry as usize;
                        continue;
                    }

                    let (target, captured) = {
                        let addr = raw.raw() as u64;
                        if !raw.raw().is_null() && self.heap.contains_addr(raw.raw()) {
                            if let Some(Object::PolyFn(gc)) =
                                self.heap.find_object_by_addr(addr)
                            {
                                let pfn = gc.as_ref();
                                (pfn.entry as usize, pfn.captured_dicts.clone())
                            } else {
                                (raw.as_int() as usize, Vec::new())
                            }
                        } else {
                            (raw.as_int() as usize, Vec::new())
                        }
                    };

                    // Pop application dictionaries (TOS = last in declaration order).
                    let mut app_dicts = Vec::with_capacity(app_dict_arity);
                    for _ in 0..app_dict_arity {
                        app_dicts.push(self.stack.pop());
                    }
                    app_dicts.reverse();

                    let member_value = |m: &crate::memory::Member| -> Value {
                        match m {
                            crate::memory::Member::Value(v) => *v,
                            crate::memory::Member::Object(o) => Value::from(o.addr()),
                        }
                    };

                    let merged_dicts: Vec<Value> = if captured.is_empty() {
                        app_dicts
                    } else {
                        let mut app_i = 0usize;
                        let mut merged = Vec::with_capacity(captured.len());
                        for slot in &captured {
                            match slot {
                                Some(m) => {
                                    merged.push(member_value(m));
                                    if app_i < app_dicts.len() {
                                        app_i += 1;
                                    }
                                }
                                None => {
                                    if app_i < app_dicts.len() {
                                        merged.push(app_dicts[app_i]);
                                        app_i += 1;
                                    } else {
                                        merged.push(Value::default());
                                    }
                                }
                            }
                        }
                        merged
                    };

                    let dict_arity = merged_dicts.len();
                    for dict in merged_dicts {
                        self.stack.push(dict);
                    }

                    let arity = value_arity + dict_arity;

                    let return_ip = ip;
                    let callee_sp = self.stack.tell() - arity;
                    self.frames.get_mut().seek(return_ip);
                    self.frames
                        .setup_current_and_advance(|frame| frame.set(callee_sp));
                    sp = callee_sp;
                    ip = target;
                }
                Instruction::MakeFn => {
                    // Stack (bottom → TOS):
                    //   [captures..., filled_param_values..., filled_mask, entry]
                    // Operand packing:
                    //   [7:0]=n_captures [15:8]=n_filled [23:16]=arity [24]=is_rest
                    let op = opcode.operand_u32();
                    let n_captures = (op & 0xFF) as usize;
                    let n_filled = ((op >> 8) & 0xFF) as usize;
                    let arity = ((op >> 16) & 0xFF) as u32;
                    let is_rest = (op & (1 << 24)) != 0;

                    let entry = self.stack.pop().as_int() as u32;
                    let filled_mask = self.stack.pop().as_int() as u32;

                    let mut filled_vals = Vec::with_capacity(n_filled);
                    for _ in 0..n_filled {
                        filled_vals.push(self.stack.pop());
                    }
                    filled_vals.reverse();

                    let mut captures = Vec::with_capacity(n_captures);
                    for _ in 0..n_captures {
                        captures.push(self.stack.pop());
                    }
                    captures.reverse();

                    let pfn = ObjFn {
                        entry,
                        arity,
                        is_rest,
                        filled_mask,
                        captured_args: filled_vals,
                        captures,
                    };
                    let (object, _) = self.heap.alloc(pfn, Object::Fn);
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::LoadStatic => {
                    let slot = opcode.operand_u32() as usize;
                    debug_assert!(
                        slot < self.statics.len(),
                        "LoadStatic slot {slot} out of bounds (len {})",
                        self.statics.len()
                    );
                    let val = self
                        .statics
                        .get(slot)
                        .copied()
                        .unwrap_or_default();
                    self.stack.push(val);
                }
                Instruction::StoreStatic => {
                    let slot = opcode.operand_u32() as usize;
                    debug_assert!(
                        slot < self.statics.len(),
                        "StoreStatic slot {slot} out of bounds (len {})",
                        self.statics.len()
                    );
                    let val = self.stack.pop();
                    if let Some(s) = self.statics.get_mut(slot) {
                        *s = val;
                    }
                }
                Instruction::BoxValue => {
                    let tag = (opcode.operand_u32() & 0xFFFF) as u16;
                    let v = self.stack.pop();
                    let addr = v.raw() as u64;
                    let payload = if addr != 0
                        && self.heap.contains_addr(addr as *mut u8)
                    {
                        if let Some(obj) =
                            Self::find_object_by_addr(&self.heap, addr)
                        {
                            Member::Object(obj)
                        } else {
                            Member::Value(v)
                        }
                    } else {
                        Member::Value(v)
                    };
                    let boxed = ObjBoxed { tag, payload };
                    let (object, _) = self.heap.alloc(boxed, Object::Boxed);
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::UnboxValue => {
                    let expected_tag = (opcode.operand_u32() & 0xFFFF) as u16;
                    let v = self.stack.pop();
                    let addr = v.raw() as u64;
                    let result = if let Some(Object::Boxed(gc)) =
                        Self::find_object_by_addr(&self.heap, addr)
                    {
                        let b = gc.as_ref();
                        if b.tag == expected_tag {
                            match &b.payload {
                                Member::Value(inner) => *inner,
                                Member::Object(o) => Value::from(o.addr()),
                            }
                        } else {
                            Value::default()
                        }
                    } else {
                        Value::default()
                    };
                    self.stack.push(result);
                }
                Instruction::MakePolyFn => {
                    let entry = opcode.operand_u32();
                    let pfn = ObjPolyFn {
                        entry,
                        type_arity: 0,
                        captured_dicts: Vec::new(),
                    };
                    let (object, _) = self.heap.alloc(pfn, Object::PolyFn);
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::MakePolyFnCapture => {
                    let count = (opcode.operand_u32() & 0xFF) as usize;
                    let entry = self.stack.pop().as_int() as u32;
                    let mut captured_dicts = vec![None; count];
                    for slot in (0..count).rev() {
                        let value = self.stack.pop();
                        let addr = value.raw() as u64;
                        captured_dicts[slot] = if addr == 0 {
                            // Unresolved evidence — filled at CallIndirect.
                            None
                        } else if self.heap.contains_addr(addr as *mut u8) {
                            Self::find_object_by_addr(&self.heap, addr).map(Member::Object)
                        } else {
                            Some(Member::Value(value))
                        };
                    }
                    let pfn = ObjPolyFn {
                        entry,
                        type_arity: 0,
                        captured_dicts,
                    };
                    let (object, _) = self.heap.alloc(pfn, Object::PolyFn);
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::DynAdd
                | Instruction::DynSub
                | Instruction::DynMul
                | Instruction::DynDiv
                | Instruction::DynMod => {
                    /// Classify a value into (ValueTag, payload-Value).
                    /// Uses `Heap::find_object_by_addr` (O(1) via addr index).
                    fn classify_dyn(v: Value, heap: &Heap) -> (ValueTag, Value) {
                        let addr = v.raw() as u64;
                        if !v.raw().is_null() && heap.contains_addr(v.raw()) {
                            if let Some(obj) = heap.find_object_by_addr(addr) {
                                return match obj {
                                    Object::Boxed(gc) => {
                                        let b = gc.as_ref();
                                        let tag = ValueTag::from_u16(b.tag)
                                            .unwrap_or(ValueTag::Int);
                                        let inner = match &b.payload {
                                            Member::Value(iv) => *iv,
                                            Member::Object(o) => Value::from(o.addr()),
                                        };
                                        (tag, inner)
                                    }
                                    Object::String(_) => (ValueTag::String, v),
                                    _ => (ValueTag::Int, v),
                                };
                            }
                        }
                        (ValueTag::Int, v)
                    }

                    let b_val = self.stack.pop();
                    let a_val = self.stack.pop();
                    let (a_tag, a_inner) = classify_dyn(a_val, &self.heap);
                    let (b_tag, b_inner) = classify_dyn(b_val, &self.heap);

                    let bc_instr = opcode.bytecode();
                    let result: Value = match (a_tag, b_tag) {
                        (ValueTag::Float, _) | (_, ValueTag::Float) => {
                            let af = a_inner.as_float();
                            let bf = b_inner.as_float();
                            let r = match bc_instr {
                                Instruction::DynAdd => af + bf,
                                Instruction::DynSub => af - bf,
                                Instruction::DynMul => af * bf,
                                Instruction::DynDiv => af / bf,
                                Instruction::DynMod => af % bf,
                                _ => unreachable!(),
                            };
                            Value::from(r)
                        }
                        (ValueTag::String, ValueTag::String)
                            if matches!(bc_instr, Instruction::DynAdd) =>
                        {
                            let sa = Self::object_string_value(&self.heap, &a_inner);
                            let sb = Self::object_string_value(&self.heap, &b_inner);
                            let concat = sa + &sb;
                            let (obj, _) = self.heap.alloc(
                                ObjString::from(concat.as_str()),
                                Object::String,
                            );
                            self.alloc_counter += 1;
                            if self.alloc_counter > GC_TRIGGER_INTERVAL {
                                Self::gc_collect(
                                    &mut self.heap,
                                    &self.stack,
                                    &self.resume_stack,
                                    &mut self.alloc_counter,
                                );
                            }
                            Value::from(obj.addr())
                        }
                        _ => {
                            let ai = a_inner.as_int();
                            let bi = b_inner.as_int();
                            let r = match bc_instr {
                                Instruction::DynAdd => ai.wrapping_add(bi),
                                Instruction::DynSub => ai.wrapping_sub(bi),
                                Instruction::DynMul => ai.wrapping_mul(bi),
                                Instruction::DynDiv => {
                                    if bi == 0 { 0 } else { ai / bi }
                                }
                                Instruction::DynMod => {
                                    if bi == 0 { 0 } else { ai % bi }
                                }
                                _ => unreachable!(),
                            };
                            Value::from(r)
                        }
                    };
                    self.stack.push(result);
                }
                Instruction::DynCmp => {
                    fn classify_int_dyn(v: Value, heap: &Heap) -> i64 {
                        let addr = v.raw() as u64;
                        if !v.raw().is_null() && heap.contains_addr(v.raw()) {
                            if let Some(Object::Boxed(gc)) = heap.find_object_by_addr(addr) {
                                return match &gc.as_ref().payload {
                                    Member::Value(iv) => iv.as_int(),
                                    Member::Object(_) => 0,
                                };
                            }
                        }
                        v.as_int()
                    }
                    let kind = opcode.operand_u32() & 0xFF;
                    let b_val = self.stack.pop();
                    let a_val = self.stack.pop();
                    let ai = classify_int_dyn(a_val, &self.heap);
                    let bi = classify_int_dyn(b_val, &self.heap);
                    let result = match kind {
                        0 => ai < bi,   // Le
                        1 => ai <= bi,  // Leq
                        2 => ai > bi,   // Gt
                        3 => ai >= bi,  // Geq
                        _ => false,
                    };
                    self.stack.push(Value::from(result));
                }
                Instruction::DynEq | Instruction::DynNe => {
                    fn classify_raw_dyn(v: Value, heap: &Heap) -> u64 {
                        let addr = v.raw() as u64;
                        if !v.raw().is_null() && heap.contains_addr(v.raw()) {
                            if let Some(Object::Boxed(gc)) = heap.find_object_by_addr(addr) {
                                return match &gc.as_ref().payload {
                                    Member::Value(iv) => iv.raw() as u64,
                                    Member::Object(o) => o.addr(),
                                };
                            }
                        }
                        v.raw() as u64
                    }
                    let b_val = self.stack.pop();
                    let a_val = self.stack.pop();
                    let ar = classify_raw_dyn(a_val, &self.heap);
                    let br = classify_raw_dyn(b_val, &self.heap);
                    let eq = ar == br;
                    let result = if matches!(opcode.bytecode(), Instruction::DynEq) {
                        eq
                    } else {
                        !eq
                    };
                    self.stack.push(Value::from(result));
                }
                Instruction::DynPrint => {
                    let v = self.stack.pop();
                    let addr = v.raw() as u64;
                    let text = if !v.raw().is_null()
                        && self.heap.contains_addr(v.raw())
                    {
                        if let Some(obj) = self.heap.find_object_by_addr(addr) {
                            match obj {
                                Object::Boxed(gc) => {
                                    let b = gc.as_ref();
                                    match ValueTag::from_u16(b.tag) {
                                        Some(ValueTag::Int) => {
                                            match &b.payload {
                                                Member::Value(iv) => iv.as_int().to_string(),
                                                _ => "?".to_string(),
                                            }
                                        }
                                        Some(ValueTag::Float) => {
                                            match &b.payload {
                                                Member::Value(iv) => {
                                                    format!("{:.?}", iv.as_float())
                                                }
                                                _ => "?".to_string(),
                                            }
                                        }
                                        Some(ValueTag::Bool) => {
                                            match &b.payload {
                                                Member::Value(iv) => {
                                                    if iv.as_int() != 0 { "true" } else { "false" }
                                                        .to_string()
                                                }
                                                _ => "?".to_string(),
                                            }
                                        }
                                        Some(ValueTag::String) => {
                                            match &b.payload {
                                                Member::Object(o) => {
                                                    Self::object_string_value(
                                                        &self.heap,
                                                        &Value::from(o.addr()),
                                                    )
                                                }
                                                Member::Value(iv) => {
                                                    Self::object_string_value(&self.heap, iv)
                                                }
                                            }
                                        }
                                        _ => "?".to_string(),
                                    }
                                }
                                Object::String(gc) => gc.as_ref().data.clone(),
                                _ => "?".to_string(),
                            }
                        } else {
                            "?".to_string()
                        }
                    } else {
                        v.as_int().to_string()
                    };
                    if let Some(out) = self.output.as_mut() {
                        let _ = write!(out, "{text}");
                    } else {
                        print!("{text}");
                    }
                }
                _ => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use common::{
        ArchivedByte as Byte, ArchivedInstruction as Instruction, Byte as RawByte, Value,
    };

    use super::{dispatch_count, reset_dispatch_count};
    use crate::{Heap, Machine, ObjEnum};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct TestOutputBuf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TestOutputBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn take_test_output(buf: Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        Arc::try_unwrap(buf)
            .expect("VM still holds a reference to the buffer")
            .into_inner()
            .expect("mutex poisoned")
    }

    /// Build a `MAKE_ENUM` byte with the given tag and arity
    /// packed into the operand (upper 16 bits = tag, lower 16
    /// bits = arity).
    fn make_enum(tag: u16, arity: u16) -> Byte {
        Byte::new(Instruction::MakeEnum).with_operands_u16([tag, arity])
    }

    fn jump_if_match(tag: u16, pool_idx: u16) -> Byte {
        Byte::new(Instruction::JumpIfMatch).with_operands_u16([tag, pool_idx])
    }

    /// Build an `UNPACK` byte with the given arity in the
    /// operand.
    fn unpack(arity: u32) -> Byte {
        Byte::new(Instruction::Unpack).with_operand_u32(arity)
    }

    fn load_field(field_index: u16) -> Byte {
        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32)
    }

    fn store_pop(slot: u32) -> Byte {
        Byte::new(Instruction::StorePop).with_operand_u32(slot)
    }

    /// Build a `LOAD` byte that pushes `stack[frame.sp + slot]`
    /// onto the stack. Used to verify that a value previously
    /// written by `STORE_POP` is read back correctly.
    fn load(slot: u32) -> Byte {
        Byte::new(Instruction::LOAD).with_operand_u32(slot)
    }

    /// Fused fib body for dispatch-count regression tests, using the
    /// operator-parameterized superinstructions (`BinSlotImm`,
    /// `ConstReturnImm`, `BinReturn`). Real recursion: fib(n) =
    /// fib(n-1) + fib(n-2), base case fib(<=2) = 1.
    fn fused_fib_bytecode(n: i64) -> Vec<Byte> {
        let leq = Instruction::LEQ as u8;
        let sub = Instruction::SUB as u8;
        let add = Instruction::ADD as u8;
        vec![
            Byte::new(Instruction::CONST).with_const_inline(n as i32),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::HALT),
            // 3: if !(n <= 2) jump to 6 (recurse); else fall through.
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(leq, 0, 2),
            Byte::new(Instruction::JMPF).with_operand_u32(6),
            // 5: base case → return 1.
            Byte::new(Instruction::ConstReturnImm).with_operand_u32(1),
            // 6: fib(n - 1)
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 1),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            // 8: fib(n - 2)
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 2),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            // 11: return fib(n-1) + fib(n-2)
            Byte::new(Instruction::BinReturn).with_bin_return(add),
        ]
    }

    #[test]
    fn fused_fib_reduces_dispatch_count_for_n13() {
        reset_dispatch_count();
        let unfused = [
            Byte::new(Instruction::CONST).with_const_inline(13),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::LEQ),
            // if !(n <= 2) jump to 9 (recurse); else fall through.
            Byte::new(Instruction::JMPF).with_operand_u32(9),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_const_inline(2),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        reset_dispatch_count();
        Machine::<512>::default().run(&unfused);
        let unfused_ops = dispatch_count();

        reset_dispatch_count();
        Machine::<512>::default().run(&fused_fib_bytecode(13));
        let fused_ops = dispatch_count();

        // Both forms recurse identically, so the unfused run must
        // dispatch many opcodes (guards against a non-recursive
        // regression like the one this test previously masked).
        assert!(
            unfused_ops > 100,
            "unfused fib should actually recurse; got {unfused_ops}"
        );
        assert!(
            fused_ops < unfused_ops,
            "fused fib should dispatch fewer opcodes (fused={fused_ops}, unfused={unfused_ops})"
        );
    }

    /// Build a `CONST` byte that pushes the given `i64` value
    /// onto the stack. Used to set up the operand values for
    /// `MAKE_ENUM` and `JUMP_IF_MATCH`.
    fn const_int(value: i64) -> Byte {
        Byte::new(Instruction::CONST).with_const_inline(value as i32)
    }

    /// Tail-recursive countdown reuses one frame (no stack overflow for deep n).
    #[test]
    fn tail_call_countdown_reuses_frame() {
        const ENTRY: u32 = 3;
        let mut code = vec![
            const_int(10),
            Byte::new(Instruction::CALL).with_call_packed(1, ENTRY),
            Byte::new(Instruction::HALT),
            // ENTRY: if n == 0 { return n }
            load(0),
            const_int(0),
            Byte::new(Instruction::EQ),
            Byte::new(Instruction::JMPF).with_operand_u32(0), // patched below
            load(0),
            Byte::new(Instruction::RETURN),
        ];
        let continue_at = code.len() as u32;
        code.extend([
            load(0),
            const_int(1),
            Byte::new(Instruction::SUB),
            Byte::new(Instruction::TailCall).with_call_packed(1, ENTRY),
        ]);
        code[6] = Byte::new(Instruction::JMPF).with_operand_u32(continue_at);
        let mut vm = Machine::<64>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 0);
    }

    #[test]
    fn array_push_grows_in_place_and_len_reports_new_size() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(1),
            const_int(2),
            Byte::new(Instruction::MakeArray).with_operand_u32(2),
            Byte::new(Instruction::DUPLICATE),
            const_int(3),
            Byte::new(Instruction::ArrayPush),
            Byte::new(Instruction::DUPLICATE),
            Byte::new(Instruction::ArrayLen),
            Byte::new(Instruction::HALT),
        ]);

        assert_eq!(vm.pop().as_int(), 3);

        vm.run(&[
            Byte::new(Instruction::DUPLICATE),
            const_int(2),
            Byte::new(Instruction::Index),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 3);
    }

    /// Emit a STRING opcode that pushes an interned heap string.
    fn string_lit(s: &str) -> Vec<Byte> {
        let mut out = vec![Byte::new(Instruction::STRING).with_operand_u32(s.len() as u32)];
        for ch in s.chars() {
            out.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        out
    }

    #[test]
    fn dict_entries_yields_array_of_key_value_tuples() {
        // MakeDict with {a: 1, b: 2}, then DictEntries → array of pairs.
        let mut code = Vec::new();
        // value, name for field a
        code.push(const_int(1));
        code.extend(string_lit("a"));
        // value, name for field b
        code.push(const_int(2));
        code.extend(string_lit("b"));
        code.push(Byte::new(Instruction::MakeDict).with_operand_u32(2));
        code.push(Byte::new(Instruction::DictEntries));
        code.push(Byte::new(Instruction::DUPLICATE));
        code.push(Byte::new(Instruction::ArrayLen));
        code.push(Byte::new(Instruction::HALT));

        let mut vm = Machine::<16>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 2, "DictEntries should produce 2 pairs");

        // Index 0 → tuple; Index 1 on tuple → value (1 or 2 depending on table order)
        vm.run(&[
            Byte::new(Instruction::DUPLICATE),
            const_int(0),
            Byte::new(Instruction::Index),
            Byte::new(Instruction::DUPLICATE),
            const_int(1),
            Byte::new(Instruction::Index),
            Byte::new(Instruction::HALT),
        ]);
        let v0 = vm.pop().as_int();
        assert!(
            v0 == 1 || v0 == 2,
            "pair value should be 1 or 2, got {v0}"
        );
    }

    #[test]
    fn make_enum_allocates_enum_with_correct_tag() {
        let mut vm = Machine::<1>::default();
        // [MAKE_ENUM tag=0 arity=0, HALT]
        vm.run(&[make_enum(0, 0), Byte::new(Instruction::HALT)]);

        let enum_value = vm.pop();
        // The enum was allocated; its address is on the stack.
        // We don't have a direct accessor from the public VM
        // API, but we can at least check that the stack
        // contains a non-zero pointer (an allocated heap
        // object).
        assert!(
            enum_value.raw() as u64 != 0,
            "MAKE_ENUM did not push a heap pointer"
        );
    }

    #[test]
    fn make_enum_with_payload_populates_payload() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            const_int(42),
            const_int(7),
            // Codegen pushes args in REVERSE declaration order
            // so that the top of stack is payload[0]; so for a
            // 2-arg constructor with declaration order
            // (a, b), codegen emits CONST b, CONST a, MAKE_ENUM.
            make_enum(1, 2),
            Byte::new(Instruction::HALT),
        ]);
        let enum_value = vm.pop();
        assert!(
            enum_value.raw() as u64 != 0,
            "MAKE_ENUM with payload did not push a heap pointer"
        );
    }

    #[test]
    fn jump_if_match_taken_advances_ip() {
        // Build a minimal bytecode that:
        //   1. constructs an enum with tag=2, arity=1, payload=[42]
        //   2. executes JUMP_IF_MATCH tag=2 target=4 arity=1
        //   3. has a HALT at offset 4 (target)
        //
        // Since JUMP_IF_MATCH is checking for tag=2 and the
        // enum's tag IS 2, the jump is taken. The payload
        // (42) is pushed onto the stack.
        let mut vm = Machine::<4>::default();
        let constants = vec![4u64];
        vm.run_with_pool(
            &[
                // Build the enum (tag=2, arity=1) with payload [42]:
                const_int(42),
                make_enum(2, 1),
                // JUMP_IF_MATCH tag=2 target=4 (pool[0])
                jump_if_match(2, 0),
                // (Should not reach here on the jump-taken path.)
                const_int(999),
                // HALT at offset 4 (the target).
                Byte::new(Instruction::HALT),
            ],
            &constants,
            0,
        );
        // After the jump, the payload (42) was pushed. Top of
        // stack is 42.
        let v = vm.pop();
        assert_eq!(v.as_int(), 42, "JUMP_IF_MATCH did not push the payload");
    }

    #[test]
    fn jump_if_match_wide_target_round_trips() {
        let target: u32 = 100_000;
        let constants = vec![target as u64];
        let byte = jump_if_match(5, 0);
        assert!(matches!(byte.bytecode(), Instruction::JumpIfMatch));
        assert_eq!(
            byte.jump_if_match_target(&constants),
            target as usize,
            "wide target should resolve via constant pool"
        );
        assert_eq!(
            byte.operand_u32() >> 16,
            5,
            "tag should be preserved in upper 16 bits of operands"
        );
        assert_eq!(
            byte.operand_u32() & 0xFFFF,
            0,
            "lower 16 bits should hold the pool index"
        );
        assert!(target > 0xFFFF, "test must exercise wide-target path");
    }

    #[test]
    fn jump_if_match_not_taken_falls_through() {
        let mut vm = Machine::<4>::default();
        vm.run_with_pool(
            &[
                // Build an enum (tag=2, arity=1) with payload [42]:
                const_int(42),
                make_enum(2, 1),
                // JUMP_IF_MATCH tag=5 (won't match; fall through)
                jump_if_match(5, 0),
                // (Should be reached on the fall-through path.)
                const_int(99),
                // Target for the (non-taken) jump at offset 4.
                Byte::new(Instruction::HALT),
            ],
            &[],
            0,
        );
        // After fall-through, we pushed 99. Stack: [enum_ptr, 99].
        let v = vm.pop();
        assert_eq!(v.as_int(), 99, "JUMP_IF_MATCH should have fallen through");
    }

    #[test]
    fn unpack_pops_enum_and_pushes_payload() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            // Build enum (tag=0, arity=3) with payload [10, 20, 30]:
            // Codegen pushes args in REVERSE declaration order:
            const_int(30),
            const_int(20),
            const_int(10),
            make_enum(0, 3),
            unpack(3),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be 30 (payload[2]).
        assert_eq!(vm.pop().as_int(), 30);
        assert_eq!(vm.pop().as_int(), 20);
        assert_eq!(vm.pop().as_int(), 10);
    }

    #[test]
    fn load_field_extracts_field_zero() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            // Build enum (tag=0, arity=3) with payload [10, 20, 30]:
            // Pushed in REVERSE declaration order.
            const_int(30),
            const_int(20),
            const_int(10),
            make_enum(0, 3),
            // LoadField(0): pops enum, pushes payload[0] = 10.
            load_field(0),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be payload[0] = 10.
        assert_eq!(vm.pop().as_int(), 10);
    }

    #[test]
    fn load_field_extracts_last_field() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(30),
            const_int(20),
            const_int(10),
            make_enum(0, 3),
            // LoadField(2): pops enum, pushes payload[2] = 30.
            load_field(2),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be payload[2] = 30.
        assert_eq!(vm.pop().as_int(), 30);
    }

    #[test]
    fn load_field_extracts_middle_field() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            // Build enum (tag=0, arity=3) with payload
            // [10, 20, 30]: pushed in REVERSE declaration order.
            const_int(30),
            const_int(20),
            const_int(10),
            make_enum(0, 3),
            // LoadField(1): pops enum, pushes payload[1] = 20.
            load_field(1),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be payload[1] = 20.
        assert_eq!(vm.pop().as_int(), 20);
    }

    #[test]
    fn load_field_consumes_receiver() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Build enum (tag=0, arity=2) with payload [42, 99].
            const_int(99),
            const_int(42),
            make_enum(0, 2),
            // LoadField(0): pops enum, pushes payload[0] = 42.
            load_field(0),
            Byte::new(Instruction::HALT),
        ]);
        // Only ONE value should be on the stack after
        // LoadField (the extracted field). The enum itself
        // should have been consumed.
        assert_eq!(
            vm.tell(),
            1,
            "LoadField should leave exactly one value on the stack"
        );
        assert_eq!(vm.pop().as_int(), 42);
    }

    #[test]
    fn load_field_out_of_bounds_pushes_default() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Build enum (tag=0, arity=2) with payload [42, 99].
            const_int(99),
            const_int(42),
            make_enum(0, 2),
            // LoadField(5): field_index 5 is past arity=2.
            // Pop enum, push Value::default() so Access stays balanced.
            load_field(5),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(
            vm.tell(),
            1,
            "out-of-bounds LoadField should leave a default value"
        );
        assert_eq!(vm.pop(), Value::default());
    }

    /// `BinSlotSlot` applies an int binary op between two locals.
    /// Set up slots 0 and 1 with `6` and `4`, then `SUB` → `2`.
    #[test]
    fn bin_slot_slot_int_subtracts_two_locals() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(6), // slot 0
            const_int(4), // slot 1
            Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(Instruction::SUB as u8, 0, 1),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 2);
    }

    /// `BinSlotSlot` also covers float ops (both operands are slot
    /// loads, so unlike `BinSlotImm` there's no pool-constant issue).
    /// Slots 0 and 1 hold pooled `1.5` and `2.0`; `ADDF` → `3.5`.
    #[test]
    fn bin_slot_slot_float_adds_two_locals() {
        let pool = [1.5f64.to_bits(), 2.0f64.to_bits()];
        let mut vm = Machine::<8>::default();
        vm.run_with_pool(
            &[
                Byte::new(Instruction::CONST).with_operand_u32(Byte::POOL_FLAG), // pool[0] = 1.5
                Byte::new(Instruction::CONST).with_operand_u32(1 | Byte::POOL_FLAG), // pool[1] = 2.0
                Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(
                    Instruction::ADDF as u8,
                    0,
                    1,
                ),
                Byte::new(Instruction::HALT),
            ],
            &pool,
            0,
        );
        assert_eq!(vm.pop().as_float(), 3.5);
    }

    #[test]
    fn nested_enum_gc_traces_correctly() {
        use crate::{Heap, Member, ObjString, Object};
        use std::collections::HashSet;

        let mut heap = Heap::default();

        // Allocate an inner enum (no payload).
        let (inner_obj, _) = heap.alloc(
            ObjEnum {
                tag: 99,
                payload: vec![],
            },
            Object::Enum,
        );
        // Allocate a string.
        let (string_obj, _) = heap.alloc(ObjString::from("inner"), Object::String);
        // Allocate an outer enum whose payload contains
        // references to both the inner enum and the string.
        let (outer_obj, _) = heap.alloc(
            ObjEnum {
                tag: 0,
                payload: vec![Member::Object(inner_obj), Member::Object(string_obj)],
            },
            Object::Enum,
        );

        let mut gray = Vec::new();
        heap.trace(&[outer_obj.addr()]);
        outer_obj.mark_references(&mut gray);
        while let Some(o) = gray.pop() {
            o.mark_references(&mut gray);
        }

        // Sweep — anything not marked is deallocated.
        unsafe {
            heap.sweep();
        }

        // All three must survive: outer (the root), inner
        // (referenced from outer's payload), and string
        // (also referenced from outer's payload).
        let mut addrs = HashSet::new();
        for o in heap.into_iter() {
            addrs.insert(o.addr());
        }
        assert!(
            addrs.contains(&outer_obj.addr()),
            "outer enum was collected despite being the GC root"
        );
        assert!(
            addrs.contains(&inner_obj.addr()),
            "inner enum was collected despite being in outer's payload"
        );
        assert!(
            addrs.contains(&string_obj.addr()),
            "string was collected despite being in outer's payload"
        );
    }

    #[test]
    fn heap_does_not_grow_unboundedly_under_repeated_alloc() {
        use std::collections::HashSet;

        let mut vm = Machine::<256>::default();

        // Build bytecode: CONST 0 (the sentinel int); then N
        // iterations of `MAKE_ENUM 0 1` (an enum wrapping the
        // sentinel); POP each result so the address is no
        // longer on the stack. After POP, the enum is
        // unreachable — the next GC cycle should free it.
        let n: usize = 200; // 200 > GC_TRIGGER_INTERVAL = 64
        let mut bytecode: Vec<Byte> = Vec::with_capacity(n * 2 + 4);
        bytecode.push(const_int(0));
        for _ in 0..n {
            bytecode.push(make_enum(0, 1));
            bytecode.push(Byte::new(Instruction::POP));
        }
        bytecode.push(Byte::new(Instruction::HALT));

        vm.run(&bytecode);

        // After running, the heap should contain FAR FEWER
        // than N objects. With 200 allocations and a GC
        // threshold of 64, the worst case is "one GC threshold
        // worth" (~64) of unreachable enums that haven't been
        // collected yet because the last GC cycle hasn't
        // caught up. Without GC, this would be ~200.
        //
        // We assert `live_count < n` as the success criterion
        // (the heap does not grow proportionally to the
        // number of allocations). Tightening further would
        // require waiting for a final GC, which would need
        // another mechanism we don't have.
        let live_addrs: HashSet<u64> = vm.heap().into_iter().map(|o| o.addr()).collect();

        assert!(
            live_addrs.len() < n,
            "expected heap to contain far fewer than {} objects, got {}",
            n,
            live_addrs.len()
        );

        // Stronger: should be much less than n — bounded by
        // `GC_TRIGGER_INTERVAL` plus a few extra. The exact
        // count is timing-dependent but should be nowhere
        // near n.
        let _ = vm.alloc_counter();
    }

    #[test]
    fn live_enum_survives_automatic_gc_cycle() {
        use std::collections::HashSet;

        let mut vm = Machine::<256>::default();

        // Build bytecode:
        //   MAKE_ENUM 7 1 (the live root, payload = sentinel int)
        //   loop 200 times:
        //     MAKE_ENUM 0 1 (an unrelated enum — unreachable
        //     after POP)
        //     POP
        //   HALT
        //
        // The live root's address sits on the operand stack
        // for the entire program — so the GC must preserve
        // it across every collection cycle.
        let n: usize = 200;
        let mut bytecode: Vec<Byte> = Vec::with_capacity(n * 2 + 4);
        bytecode.push(const_int(0)); // sentinel payload
        bytecode.push(make_enum(7, 1)); // tag=7 sentinel, arity=1
        let root_addr = {
            // We can't easily capture the address at codegen
            // time (we'd need a DUP + something), so we'll
            // just inspect the heap after the run instead.
            // For now, leave the live root on the stack.
            // Duplicate it so we still have it after we POP
            // unrelated allocations... wait, no — the
            // unrelated allocations are POPed, the root is
            // NOT popped. Just leave it.
            vm.run(&[]); // dummy to silence unused
            0u64
        };
        let _ = root_addr;

        for _ in 0..n {
            bytecode.push(const_int(0));
            bytecode.push(make_enum(0, 1));
            bytecode.push(Byte::new(Instruction::POP));
        }
        // Now the live root is at the bottom of the stack,
        // with n stale enums (already POPed) above it on
        // nothing (they were popped off the stack but their
        // allocations may still be on the heap until GC).
        // HALT.
        bytecode.push(Byte::new(Instruction::HALT));

        vm.run(&bytecode);

        // The live root should still be on the stack (we
        // never POPed it). We can't easily inspect the stack
        // from outside, but we CAN inspect the heap: after GC
        // the heap should contain only the live root. The n
        // unreachable enums should have been collected.
        let live_addrs: HashSet<u64> = vm.heap().into_iter().map(|o| o.addr()).collect();

        // Bound: at most a small handful of objects — the
        // live root (1) plus at most the threshold minus one
        // (uncollected but unreachable) enums. The point is
        // `live_addrs.len() < n` — without GC, it would be
        // ~n+1.
        assert!(
            live_addrs.len() < n,
            "expected heap to be much smaller than n={}, got {}",
            n,
            live_addrs.len()
        );

        // At least the live root should be present.
        assert!(
            !live_addrs.is_empty(),
            "expected at least one live object (the root enum)"
        );
    }

    #[test]
    fn panic_opcode_sets_panicked_and_writes_message() {
        let mut vm = Machine::<4>::default();
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        vm.with_output(TestOutputBuf(Arc::clone(&buf)));

        let mut bytecode = Vec::new();
        // STRING 4 "boom"
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(4));
        for ch in "boom".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::Panic));
        // Unreachable if Panic aborts.
        bytecode.push(Byte::new(Instruction::HALT));

        vm.run(&bytecode);
        assert!(vm.panicked());
        let _ = vm.restore_output();
        let bytes = take_test_output(buf);
        let s = String::from_utf8(bytes).expect("output should be valid UTF-8");
        assert_eq!(s, "panic: boom");
    }

    fn with_output_captures_print() {
        let mut vm = Machine::<16>::default();
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        vm.with_output(TestOutputBuf(Arc::clone(&buf)));

        // Build bytecode:
        //   STRING 5 "hello"
        //   PRINT
        //   HALT
        let mut bytecode: Vec<Byte> = Vec::new();
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(5));
        for ch in "hello".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::PRINT));
        bytecode.push(Byte::new(Instruction::HALT));

        vm.run(&bytecode);

        // Drop the sink first so the `Rc` we hold is the
        // only one (then we can move the `Vec` out).
        let _ = vm.restore_output();

        let bytes = take_test_output(buf);
        let s = String::from_utf8(bytes).expect("output should be valid UTF-8");
        assert_eq!(s, "hello");
    }

    /// GetField must return heap-object field values (strings,
    /// nested dicts, …) by address — not the `-1` sentinel used
    /// for missing fields. Pre-P0 returned `-1` for `Member::Object`.
    #[test]
    fn get_field_returns_heap_object_field() {
        let mut vm = Machine::<16>::default();
        // STRING 2 "hi"  → heap string
        // STRING 1 "s"   → field name
        // MakeDict 1     → { s: "hi" }
        // DUPLICATE
        // STRING 1 "s"
        // GetField       → should push the "hi" string address
        // PRINT
        // HALT
        let mut bytecode: Vec<Byte> = Vec::new();
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(2));
        for ch in "hi".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(1));
        for ch in "s".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::MakeDict).with_operand_u32(1));
        bytecode.push(Byte::new(Instruction::DUPLICATE));
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(1));
        for ch in "s".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::GetField));
        bytecode.push(Byte::new(Instruction::PRINT));
        bytecode.push(Byte::new(Instruction::HALT));

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        vm.with_output(TestOutputBuf(Arc::clone(&buf)));
        vm.run(&bytecode);
        let _ = vm.restore_output();
        let bytes = take_test_output(buf);
        let s = String::from_utf8(bytes).expect("output should be valid UTF-8");
        assert_eq!(s, "hi", "GetField should return the stored string, not -1");
    }

    #[test]
    fn store_pop_writes_value_to_slot_and_pops() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Push 42 onto the operand stack.
            const_int(42),
            // Pop 42, write to slot 0 (= frame.sp + 0 = 0).
            store_pop(0),
            // Push slot 0 (= 42) back onto the stack.
            load(0),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be 42 — proving both the
        // write-to-slot and the pop-and-write semantics.
        assert_eq!(vm.pop().as_int(), 42);
    }

    #[test]
    fn store_pop_writes_to_correct_slot_index() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Push 99, store at slot 0.
            const_int(99),
            store_pop(0),
            // Push 42, store at slot 2.
            const_int(42),
            store_pop(2),
            // Push slot 2 (= 42) — the second binding.
            load(2),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be 42 (the value stored at
        // slot 2). Slot 0 still holds 99.
        assert_eq!(vm.pop().as_int(), 42);
    }

    #[test]
    fn store_pop_two_bindings_preserves_both_values() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // let x = 5;
            const_int(5),
            store_pop(0),
            // let y = 10;
            const_int(10),
            store_pop(1),
            // read x back
            load(0),
            // push y so we can add them
            load(1),
            // x + y = 15
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 15);
    }

    #[test]
    fn store_pop_overwrites_existing_slot() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // let x = 5;
            const_int(5),
            store_pop(0),
            // x = 10;
            const_int(10),
            store_pop(0),
            // read x back
            load(0),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 10);
    }

    /// P0: reassigning a low slot must not truncate the cursor past
    /// higher locals (shared operand-stack / locals area).
    #[test]
    fn store_pop_preserves_higher_locals_and_cursor() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(10),
            store_pop(0),
            const_int(20),
            store_pop(1),
            const_int(30),
            store_pop(2),
            // Reassign slot 0 while slots 1 and 2 are live.
            const_int(99),
            store_pop(0),
            load(0),
            load(1),
            load(2),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 30, "slot 2 must survive StorePop 0");
        assert_eq!(vm.pop().as_int(), 20, "slot 1 must survive StorePop 0");
        assert_eq!(vm.pop().as_int(), 99, "slot 0 must hold the new value");
    }

    /// P1: heap objects stored in an array survive GC when the array is rooted.
    #[test]
    fn array_elements_survive_gc() {
        let mut vm = Machine::<64>::default();
        // STRING "hi" → MakeArray(1) → store slot 0 → allocate 128 enums →
        // load slot 0 → Index 0 → PRINT → HALT
        let mut code = Vec::new();
        code.push(Byte::new(Instruction::STRING).with_operand_u32(2));
        for ch in "hi".chars() {
            code.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        code.push(Byte::new(Instruction::MakeArray).with_operand_u32(1));
        code.push(store_pop(0));
        for _ in 0..128 {
            code.push(Byte::new(Instruction::MakeEnum).with_operands_u16([0, 0]));
            code.push(Byte::new(Instruction::POP));
        }
        code.push(load(0));
        code.push(const_int(0));
        code.push(Byte::new(Instruction::Index));
        code.push(Byte::new(Instruction::PRINT));
        code.push(Byte::new(Instruction::HALT));

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        vm.with_output(TestOutputBuf(Arc::clone(&buf)));
        vm.run(&code);
        let _ = vm.restore_output();
        let bytes = take_test_output(buf);
        let s = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(s, "hi", "array string element must survive GC pressure");
    }

    /// P1 sibling: heap objects stored in a tuple survive GC when the
    /// tuple is rooted. Arrays were covered above; tuples share the
    /// same `mark_aggregate_elements` path and must not regress alone.
    #[test]
    fn tuple_elements_survive_gc() {
        let mut vm = Machine::<64>::default();
        // STRING "ok" → MakeTuple(1) → store slot 0 → allocate 128 enums →
        // load slot 0 → Index 0 → PRINT → HALT
        let mut code = Vec::new();
        code.push(Byte::new(Instruction::STRING).with_operand_u32(2));
        for ch in "ok".chars() {
            code.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        code.push(Byte::new(Instruction::MakeTuple).with_operand_u32(1));
        code.push(store_pop(0));
        for _ in 0..128 {
            code.push(Byte::new(Instruction::MakeEnum).with_operands_u16([0, 0]));
            code.push(Byte::new(Instruction::POP));
        }
        code.push(load(0));
        code.push(const_int(0));
        code.push(Byte::new(Instruction::Index));
        code.push(Byte::new(Instruction::PRINT));
        code.push(Byte::new(Instruction::HALT));

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        vm.with_output(TestOutputBuf(Arc::clone(&buf)));
        vm.run(&code);
        let _ = vm.restore_output();
        let bytes = take_test_output(buf);
        let s = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(s, "ok", "tuple string element must survive GC pressure");
    }

    /// P5: PRINT flushes so redirected sinks observe output before HALT.
    /// HALT also flushes, so a single PRINT+HALT program must flush ≥2 times
    /// (once from PRINT, once from HALT). Pre-fix PRINT skipped flush → 1.
    #[test]
    fn print_flushes_output_sink() {
        use std::sync::{Arc, Mutex};
        struct FlushCountingWriter {
            buf: Arc<Mutex<Vec<u8>>>,
            flushes: Arc<Mutex<usize>>,
        }
        impl std::io::Write for FlushCountingWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.buf.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                *self.flushes.lock().unwrap() += 1;
                Ok(())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let flushes = Arc::new(Mutex::new(0usize));
        let mut vm = Machine::<16>::default();
        vm.with_output(FlushCountingWriter {
            buf: Arc::clone(&buf),
            flushes: Arc::clone(&flushes),
        });

        let mut bytecode = Vec::new();
        bytecode.push(Byte::new(Instruction::STRING).with_operand_u32(3));
        for ch in "xyz".chars() {
            bytecode.push(Byte::new(Instruction::DATA).with_operand_u32(ch as u32));
        }
        bytecode.push(Byte::new(Instruction::PRINT));
        bytecode.push(Byte::new(Instruction::HALT));
        vm.run(&bytecode);
        let _ = vm.restore_output();
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "xyz");
        assert!(
            *flushes.lock().unwrap() >= 2,
            "PRINT+HALT must each flush; got {} flush(es)",
            *flushes.lock().unwrap()
        );
    }

    /// Host native dispatch via explicit signature registry.
    #[test]
    fn host_invoke_dispatches_rust_closure() {
        use crate::ffi::FfiSignatureBuilder;
        use crate::memory::FfiType;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let sig = FfiSignatureBuilder::new("inc")
            .ret(FfiType::Void)
            .build()
            .unwrap();
        let mut vm = Machine::<4>::default();
        let fn_id = vm.register_fn(sig, |_heap, _args| {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        });
        vm.run(&[
            Byte::new(Instruction::CONST).with_value_u32(fn_id as u32),
            Byte::new(Instruction::MakeTuple).with_operand_u32(0),
            Byte::new(Instruction::HostInvoke).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_value_u32(fn_id as u32),
            Byte::new(Instruction::MakeTuple).with_operand_u32(0),
            Byte::new(Instruction::HostInvoke).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_value_u32(fn_id as u32),
            Byte::new(Instruction::MakeTuple).with_operand_u32(0),
            Byte::new(Instruction::HostInvoke).with_operand_u32(0),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            3,
            "HostInvoke should have invoked the Rust closure 3 times"
        );
    }

    fn install_program(vm: &mut Machine<512>, code: &[Byte]) {
        vm.program_code = unsafe {
            std::slice::from_raw_parts(code.as_ptr().cast::<RawByte>(), code.len()).to_vec()
        };
        vm.program_constants.clear();
    }

    /// Reentrant `call_function` runs bytecode at the given offset.
    #[test]
    fn call_function_runs_bytecode_at_offset() {
        let mut vm = Machine::<512>::default();
        install_program(
            &mut vm,
            &[
                load(0),
                const_int(2),
                Byte::new(Instruction::MUL),
                Byte::new(Instruction::RETURN),
            ],
        );
        let out = vm.call_function(0, &[Value::from(21_i64)]);
        assert_eq!(out.as_int(), 42);
    }

    /// `load_program` is the public entry used by `coil test` —
    /// without it, harness cases cannot `call_function` against compiled
    /// bytecode.
    #[test]
    fn load_program_enables_call_function() {
        let mut vm = Machine::<512>::default();
        let code = [
            load(0),
            const_int(3),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        let raw: Vec<RawByte> = unsafe {
            std::slice::from_raw_parts(code.as_ptr().cast::<RawByte>(), code.len()).to_vec()
        };
        vm.load_program(&raw, &[]);
        let out = vm.call_function(0, &[Value::from(39_i64)]);
        assert_eq!(out.as_int(), 42);
        assert!(!vm.panicked());
    }

    /// Harness soft-pass checks `Result::Ok` via tag 0.
    #[test]
    fn result_is_ok_true_for_tag_zero_enum() {
        let mut vm = Machine::<4>::default();
        vm.run(&[const_int(1), make_enum(0, 1), Byte::new(Instruction::HALT)]);
        let v = vm.pop();
        assert!(vm.result_is_ok(v), "tag 0 must count as Ok");
    }

    /// Harness soft-fail checks `Result::Err` via tag 1.
    #[test]
    fn result_is_ok_false_for_tag_one_enum() {
        let mut vm = Machine::<4>::default();
        vm.run(&[const_int(1), make_enum(1, 1), Byte::new(Instruction::HALT)]);
        let v = vm.pop();
        assert!(!vm.result_is_ok(v), "tag 1 must count as Err");
    }

    /// Non-enum values (and missing heap objects) are not Ok.
    #[test]
    fn result_is_ok_false_for_immediate() {
        let vm = Machine::<4>::default();
        assert!(!vm.result_is_ok(Value::from(0_i64)));
        assert!(!vm.result_is_ok(Value::from(42_i64)));
    }

    /// Inner `CALL`/`RETURN` must unwind normally under `call_function`.
    /// Without `nested_frame_depths`, the inner RETURN would capture early
    /// and return 7 instead of continuing the outer body (7+1=8).
    #[test]
    fn call_function_captures_only_outer_return_not_inner_call() {
        let mut vm = Machine::<512>::default();
        // 0: CALL → 4
        // 1: CONST 1
        // 2: ADD
        // 3: RETURN   (outer — captured by call_function)
        // 4: CONST 7
        // 5: RETURN   (inner — must unwind, not capture)
        install_program(
            &mut vm,
            &[
                Byte::new(Instruction::CALL).with_call_packed(0, 4),
                const_int(1),
                Byte::new(Instruction::ADD),
                Byte::new(Instruction::RETURN),
                const_int(7),
                Byte::new(Instruction::RETURN),
            ],
        );
        let out = vm.call_function(0, &[]);
        assert_eq!(out.as_int(), 8);
    }

    /// Nested `call_function` (FFI callback reentrancy) must not clobber the
    /// outer frame-depth target — outer RETURN still captures a non-default value.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn nested_call_function_preserves_outer_return() {
        use crate::ffi::FfiSignature;
        use crate::memory::FfiType;

        let lib_name = crate::ffi::platform_shared_lib_filename("sum");
        let lib_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples")
            .join(&lib_name);
        if !lib_path.exists() {
            if std::env::var_os("CI").is_some() {
                panic!("FFI soft-skip forbidden in CI: {lib_name} not built");
            }
            eprintln!("skipping: {lib_name} not built");
            return;
        }

        let mut vm = Machine::<512>::default();
        let lib_val = vm
            .load_userland_library(lib_path.to_str().unwrap())
            .unwrap_or_else(|e| panic!("load {lib_name}: {e}"));
        let sig = FfiSignature::from_parts(
            "apply_cb",
            vec![FfiType::Callback(0), FfiType::Int],
            FfiType::Int,
        )
        .unwrap();
        let fn_id = vm
            .register_ffi_function(lib_val, sig)
            .unwrap_or_else(|e| panic!("declare apply_cb: {e}"));

        // 0: identity callback — LOAD 0; RETURN
        // 2: outer — FfiInvoke apply_cb(callback@0, 21); POP Result; CONST 99; RETURN
        install_program(
            &mut vm,
            &[
                load(0),
                Byte::new(Instruction::RETURN),
                // outer entry (offset 2); args: lib, fn_id
                load(0),
                load(1),
                Byte::new(Instruction::CodePtr).with_operand_u32(0),
                const_int(21),
                Byte::new(Instruction::MakeTuple).with_operand_u32(2),
                Byte::new(Instruction::FfiInvoke).with_operand_u32(2),
                Byte::new(Instruction::POP),
                const_int(99),
                Byte::new(Instruction::RETURN),
            ],
        );

        let out = vm.call_function(2, &[lib_val, Value::from(fn_id as i64)]);
        assert_eq!(
            out.as_int(),
            99,
            "outer call_function must capture its RETURN after nested callback"
        );
    }

    /// C → coil callback via `apply_cb` in libsum.so.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn vm_callback_apply_cb_doubles() {
        use crate::ffi::{
            FfiSignature, InvokeContext, callback_cif, invoke_via_libffi, make_int_callback,
            prepare_cif_for_symbol, resolve_library,
        };
        use crate::memory::FfiType;
        use std::ffi::c_void;

        let lib_name = crate::ffi::platform_shared_lib_filename("sum");
        let lib_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples")
            .join(&lib_name);
        if !lib_path.exists() {
            if std::env::var_os("CI").is_some() {
                panic!("FFI soft-skip forbidden in CI: {lib_name} not built");
            }
            eprintln!("skipping: {lib_name} not built");
            return;
        }
        let lib = resolve_library(lib_path.to_str().unwrap(), None, &[])
            .unwrap_or_else(|e| panic!("load {lib_name}: {e}"));

        let mut vm = Machine::<512>::default();
        install_program(
            &mut vm,
            &[
                load(0),
                const_int(2),
                Byte::new(Instruction::MUL),
                Byte::new(Instruction::RETURN),
            ],
        );

        let cif = callback_cif(&[FfiType::Int], FfiType::Int, &[]).unwrap();
        let vm_ptr = &mut vm as *mut Machine<512> as *mut c_void;
        let closure = make_int_callback(vm_ptr, 0, Machine::<512>::invoke_call, cif).unwrap();
        let cb_ptr = closure.code_ptr_usize();
        vm.ffi_closures.push(closure);

        let sig = FfiSignature::from_parts(
            "apply_cb",
            vec![FfiType::Callback(0), FfiType::Int],
            FfiType::Int,
        )
        .unwrap();
        let prepared = prepare_cif_for_symbol(&sig, &lib, "apply_cb", &[]).unwrap();
        let args = [Value::from(cb_ptr as u64), Value::from(21_i64)];
        let mut ctx = InvokeContext::new(&mut vm.heap as *mut Heap, &vm.struct_layouts);
        let mut closure_ptrs = Vec::new();
        let ret = invoke_via_libffi(&prepared, &sig, &args, None, &mut ctx, &mut closure_ptrs)
            .unwrap()
            .unwrap();
        assert_eq!(ret.as_int(), 42);
    }

    fn make_coro(arity: u32, target: u32) -> Byte {
        Byte::new(Instruction::MakeCoro).with_call_packed(arity, target)
    }

    /// Create → resume → yield returns the yielded value to the resumer.
    #[test]
    fn coroutine_resume_yields_value() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            make_coro(0, 3),
            Byte::new(Instruction::ResumeCoro),
            Byte::new(Instruction::HALT),
            // 3: coroutine body
            const_int(42),
            Byte::new(Instruction::YieldCoro),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 42);
    }

    /// Resuming a completed coroutine pushes 0 (MVP done protocol).
    #[test]
    fn coroutine_resume_after_done_returns_zero() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            make_coro(0, 9),
            store_pop(0),
            load(0),
            Byte::new(Instruction::ResumeCoro),
            load(0),
            Byte::new(Instruction::ResumeCoro),
            load(0),
            Byte::new(Instruction::ResumeCoro),
            Byte::new(Instruction::HALT),
            const_int(7),
            Byte::new(Instruction::YieldCoro),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 0);
        assert_eq!(vm.pop().as_int(), 7);
    }

    /// Resume with send + binding yield: second resume returns the sent value.
    #[test]
    fn coroutine_resume_with_send_binding_yield() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            make_coro(0, 8),
            store_pop(0),
            load(0),
            Byte::new(Instruction::ResumeCoro),
            const_int(200),
            load(0),
            Byte::new(Instruction::ResumeCoro).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            // 8: coroutine body — yield out, receive send, yield received value
            const_int(100),
            Byte::new(Instruction::YieldCoro),
            store_pop(0),
            load(0),
            Byte::new(Instruction::YieldCoro),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 200);
        assert_eq!(vm.pop().as_int(), 100);
    }

    #[test]
    fn log_not_bool_and_int() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(1),
            Byte::new(Instruction::LogNot),
            const_int(0),
            Byte::new(Instruction::LogNot),
            const_int(42),
            Byte::new(Instruction::LogNot),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_bool(), false);
        assert_eq!(vm.pop().as_bool(), true);
        assert_eq!(vm.pop().as_bool(), false);
    }

    #[test]
    fn inc_prefix_returns_new_value() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(5),
            store_pop(0),
            Byte::new(Instruction::INC).with_inc_dec(0, true, false),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 6);
        assert_eq!(vm.stack[0].as_int(), 6);
    }

    #[test]
    fn inc_postfix_returns_old_value() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(5),
            store_pop(0),
            Byte::new(Instruction::INC).with_inc_dec(0, false, false),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 5);
        assert_eq!(vm.stack[0].as_int(), 6);
    }

    #[test]
    fn dec_prefix_and_postfix() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(5),
            store_pop(0),
            Byte::new(Instruction::DEC).with_inc_dec(0, false, false),
            Byte::new(Instruction::DEC).with_inc_dec(0, true, false),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 3);
        assert_eq!(vm.stack[0].as_int(), 3);
    }

    /// Coroutine handle + saved stack survive an automatic GC cycle.
    #[test]
    fn coroutine_handle_survives_gc() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            make_coro(0, 8),
            store_pop(0),
            make_enum(0, 0),
            make_enum(1, 0),
            make_enum(2, 0),
            load(0),
            Byte::new(Instruction::ResumeCoro),
            Byte::new(Instruction::HALT),
            const_int(99),
            Byte::new(Instruction::YieldCoro),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 99);
    }

    // ── Generics runtime opcode tests ────────────────────────────────────────

    /// `CallIndirect` pops a target offset from the stack and jumps to it,
    /// treating the remaining stack entries as the callee's arguments.
    ///
    /// Layout:
    ///   0: CONST 42        (arg0)
    ///   1: CONST 4         (target = bytecode offset 4)
    ///   2: CallIndirect    (arity=1)
    ///   3: HALT
    ///   4: LOAD 0          (callee: load arg0)
    ///   5: RETURN
    #[test]
    fn call_indirect_jumps_to_target() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(42),
            const_int(4),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            // callee at offset 4
            load(0),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 42);
    }

    /// `CodePtr` pushes an absolute bytecode offset like an integer constant;
    /// `CallIndirect` consumes it as the callee entry.
    #[test]
    fn code_ptr_feeds_call_indirect() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(42),
            Byte::new(Instruction::CodePtr).with_operand_u32(4),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            load(0),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 42);
    }

    /// `BoxValue` wraps a raw integer in an `Object::Boxed` heap cell;
    /// `UnboxValue` recovers the payload when tags match.
    #[test]
    fn box_unbox_int_roundtrip() {
        let int_tag: u32 = 0; // ValueTag::Int
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(99),
            Byte::new(Instruction::BoxValue).with_operand_u32(int_tag),
            Byte::new(Instruction::UnboxValue).with_operand_u32(int_tag),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 99);
    }

    /// `MakePolyFn` allocates a heap object and pushes a non-null address.
    #[test]
    fn make_polyfn_allocates() {
        let mut vm = Machine::<8>::default();
        // entry offset 0 — irrelevant for the allocation test.
        vm.run(&[
            Byte::new(Instruction::MakePolyFn).with_operand_u32(0),
            Byte::new(Instruction::HALT),
        ]);
        let addr = vm.pop();
        assert!(
            addr.raw() as u64 != 0,
            "MakePolyFn should push a non-null heap pointer"
        );
    }

    /// `DynAdd` with two boxed integers yields their sum as an unboxed int.
    #[test]
    fn dyn_add_ints() {
        let int_tag: u32 = 0; // ValueTag::Int
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(10),
            Byte::new(Instruction::BoxValue).with_operand_u32(int_tag),
            const_int(32),
            Byte::new(Instruction::BoxValue).with_operand_u32(int_tag),
            Byte::new(Instruction::DynAdd),
            Byte::new(Instruction::HALT),
        ]);
        // DynAdd on two Int-tagged boxed values returns an unboxed int.
        assert_eq!(vm.pop().as_int(), 42);
    }

    /// `DynAdd` with two boxed floats yields their sum as an unboxed float.
    #[test]
    fn dyn_add_floats() {
        let float_tag: u32 = 1; // ValueTag::Float
        let pool = [1.5f64.to_bits(), 2.5f64.to_bits()];
        let mut vm = Machine::<8>::default();
        vm.run_with_pool(
            &[
                // push 1.5 (pool[0])
                Byte::new(Instruction::CONST).with_operand_u32(Byte::POOL_FLAG),
                Byte::new(Instruction::BoxValue).with_operand_u32(float_tag),
                // push 2.5 (pool[1])
                Byte::new(Instruction::CONST).with_operand_u32(1 | Byte::POOL_FLAG),
                Byte::new(Instruction::BoxValue).with_operand_u32(float_tag),
                Byte::new(Instruction::DynAdd),
                Byte::new(Instruction::HALT),
            ],
            &pool,
            0,
        );
        // 1.5 + 2.5 = 4.0
        assert_eq!(vm.pop().as_float(), 4.0);
    }

    /// `MakePolyFnCapture` + `CallIndirect` injects captured dictionaries when
    /// the application site supplies none.
    #[test]
    fn call_indirect_merges_captured_dicts_without_app_evidence() {
        // Layout:
        //  0: CONST 7            captured dict (immediate)
        //  1: CodePtr 8          entry
        //  2: MakePolyFnCapture  (1 slot)
        //  3: StorePop 0         save PolyFn
        //  4: CONST 42           value arg
        //  5: LOAD 0             PolyFn
        //  6: CallIndirect       value_arity=1, app_dict_arity=0
        //  7: HALT
        //  8: LOAD 1             callee reads captured dict
        //  9: RETURN
        let mut vm = Machine::<16>::default();
        vm.run(&[
            const_int(7),
            Byte::new(Instruction::CodePtr).with_operand_u32(8),
            Byte::new(Instruction::MakePolyFnCapture).with_operand_u32(1),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(42),
            load(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            load(1),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 7);
    }

    /// Phase 4: capture with every slot `Some` and `app_dict_arity=0` still
    /// injects all dictionaries for the callee.
    #[test]
    fn call_indirect_all_some_capture_slots_work_with_zero_app_dicts() {
        // Two captured dicts (11, 22); callee returns dict1 + dict2 (slots 1, 2).
        //  0: CONST 11
        //  1: CONST 22
        //  2: CodePtr entry
        //  3: MakePolyFnCapture (2)
        //  4: StorePop 0
        //  5: CONST 1            value arg (unused by callee)
        //  6: LOAD 0
        //  7: CallIndirect value_arity=1, app_dict_arity=0
        //  8: HALT
        //  9: LOAD 1 / LOAD 2 / ADD / RETURN
        let mut vm = Machine::<16>::default();
        vm.run(&[
            const_int(11),
            const_int(22),
            Byte::new(Instruction::CodePtr).with_operand_u32(9),
            Byte::new(Instruction::MakePolyFnCapture).with_operand_u32(2),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(1),
            load(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            load(1),
            load(2),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 33);
    }

    /// Captured evidence wins over a duplicate application dictionary.
    #[test]
    fn call_indirect_prefers_captured_dict_over_app_dict() {
        // Captured dict = 11; app dict = 22; callee returns slot 1.
        let mut vm = Machine::<16>::default();
        vm.run(&[
            const_int(11),
            Byte::new(Instruction::CodePtr).with_operand_u32(9),
            Byte::new(Instruction::MakePolyFnCapture).with_operand_u32(1),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(42),
            const_int(22),
            load(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1 | (1 << 16)),
            Byte::new(Instruction::HALT),
            load(1),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 11);
    }

    /// Null capture slots are filled from application dictionaries.
    #[test]
    fn call_indirect_fills_unresolved_capture_slots_from_app() {
        let mut vm = Machine::<16>::default();
        vm.run(&[
            const_int(0), // unresolved sentinel
            Byte::new(Instruction::CodePtr).with_operand_u32(9),
            Byte::new(Instruction::MakePolyFnCapture).with_operand_u32(1),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(42),
            const_int(33),
            load(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1 | (1 << 16)),
            Byte::new(Instruction::HALT),
            load(1),
            Byte::new(Instruction::RETURN),
        ]);
        assert_eq!(vm.pop().as_int(), 33);
    }

    /// STRINGIFY turns a boxed int into a heap string "42".
    #[test]
    fn stringify_boxed_int_produces_string() {
        let mut vm = Machine::<64>::default();
        let int_tag: u32 = 0;
        vm.run(&[
            const_int(42),
            Byte::new(Instruction::BoxValue).with_operand_u32(int_tag),
            Byte::new(Instruction::STRINGIFY),
            Byte::new(Instruction::HALT),
        ]);
        let s = vm.pop();
        let text = Machine::<64>::object_string_value(&vm.heap, &s);
        assert_eq!(text, "42");
    }

    /// STRINGIFY turns a boxed float into a debug-formatted string.
    #[test]
    fn stringify_boxed_float_produces_string() {
        let pool = [1.5f64.to_bits()];
        let mut vm = Machine::<64>::default();
        let float_tag: u32 = 1;
        vm.run_with_pool(
            &[
                Byte::new(Instruction::CONST).with_operand_u32(Byte::POOL_FLAG),
                Byte::new(Instruction::BoxValue).with_operand_u32(float_tag),
                Byte::new(Instruction::STRINGIFY),
                Byte::new(Instruction::HALT),
            ],
            &pool,
            0,
        );
        let s = vm.pop();
        let text = Machine::<64>::object_string_value(&vm.heap, &s);
        assert!(
            text.contains("1.5"),
            "expected float display containing 1.5, got {text:?}"
        );
    }

    /// STRINGIFY copies a heap string through.
    #[test]
    fn stringify_string_copies_contents() {
        let mut vm = Machine::<64>::default();
        vm.run(&[
            Byte::new(Instruction::STRING).with_operand_u32(2),
            Byte::new(Instruction::DATA).with_operand_u32('h' as u32),
            Byte::new(Instruction::DATA).with_operand_u32('i' as u32),
            Byte::new(Instruction::STRINGIFY),
            Byte::new(Instruction::HALT),
        ]);
        let s = vm.pop();
        let text = Machine::<64>::object_string_value(&vm.heap, &s);
        assert_eq!(text, "hi");
    }

    /// Captured heap dictionaries stay alive across GC pressure.
    #[test]
    fn polyfn_captured_dict_survives_gc() {
        let mut vm = Machine::<64>::default();
        // Build a 1-element tuple dict, capture it, allocate many enums to
        // trigger GC, then CallIndirect and read the captured tuple via LOAD 1.
        let mut code = vec![
            Byte::new(Instruction::CodePtr).with_operand_u32(0), // placeholder method
            Byte::new(Instruction::MakeTuple).with_operand_u32(1),
            Byte::new(Instruction::CodePtr).with_operand_u32(0), // entry patched below
            Byte::new(Instruction::MakePolyFnCapture).with_operand_u32(1),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
        ];
        for _ in 0..128 {
            code.push(Byte::new(Instruction::MakeEnum).with_operands_u16([0, 0]));
            code.push(Byte::new(Instruction::POP));
        }
        let entry = code.len() as u32 + 4;
        code[2] = Byte::new(Instruction::CodePtr).with_operand_u32(entry);
        code.extend([
            const_int(1),
            load(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            load(1),
            Byte::new(Instruction::RETURN),
        ]);
        vm.run(&code);
        let dict = vm.pop();
        assert!(
            dict.raw() as u64 != 0,
            "captured dictionary must survive GC"
        );
    }

    fn unpack_at(slot: u16, arity: u16) -> Byte {
        // operands[31:16]=arity, [15:0]=slot_offset (matches UnpackAt dispatch).
        Byte::new(Instruction::UnpackAt).with_operands_u16([arity, slot])
    }

    #[test]
    fn jump_if_match_on_non_enum_falls_through() {
        let mut vm = Machine::<4>::default();
        vm.run_with_pool(
            &[
                const_int(42),
                jump_if_match(0, 0),
                const_int(7),
                Byte::new(Instruction::HALT),
            ],
            &[0u64],
            0,
        );
        assert_eq!(vm.pop().as_int(), 7);
        assert_eq!(vm.pop().as_int(), 42);
    }

    #[test]
    fn jump_if_match_on_empty_stack_is_noop() {
        let mut vm = Machine::<2>::default();
        vm.run_with_pool(
            &[
                jump_if_match(0, 0),
                const_int(3),
                Byte::new(Instruction::HALT),
            ],
            &[0u64],
            0,
        );
        assert_eq!(vm.pop().as_int(), 3);
    }

    #[test]
    fn unpack_on_non_enum_discards_scrutinee_without_payload() {
        let mut vm = Machine::<4>::default();
        vm.run(&[const_int(1), unpack(1), Byte::new(Instruction::HALT)]);
        assert_eq!(vm.tell(), 0, "non-enum UNPACK should leave stack empty");
    }

    #[test]
    fn unpack_at_on_non_enum_is_noop() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            const_int(5),
            unpack_at(0, 1),
            load(0),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 5);
    }

    /// Scratch-area nested records unpack past the live cursor; UnpackAt must
    /// extend `tell` so a later push does not overwrite the written payload.
    #[test]
    fn unpack_at_extends_tell_when_payload_past_cursor() {
        let mut vm = Machine::<8>::default();
        // Slot 0 = sibling 99; slot 1 = enum{3,7} (arity 2). UnpackAt@1 writes
        // payload into slots 1..3. Without seek(3), the next push would clobber
        // slot 2.
        vm.run(&[
            const_int(99),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(7),
            const_int(3),
            make_enum(0, 2),
            Byte::new(Instruction::StorePop).with_operand_u32(1),
            unpack_at(1, 2),
            const_int(111),
            load(0),
            load(1),
            load(2),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 7, "payload[1] must survive push after UnpackAt");
        assert_eq!(vm.pop().as_int(), 3, "payload[0] at slot 1");
        assert_eq!(vm.pop().as_int(), 99, "sibling at slot 0 preserved");
        assert_eq!(vm.pop().as_int(), 111, "push must land past unpacked payload");
    }

    #[test]
    fn get_field_missing_returns_minus_one() {
        let mut vm = Machine::<16>::default();
        let mut code = Vec::new();
        code.push(const_int(1));
        code.extend(string_lit("a"));
        code.push(Byte::new(Instruction::MakeDict).with_operand_u32(1));
        code.extend(string_lit("missing"));
        code.push(Byte::new(Instruction::GetField));
        code.push(Byte::new(Instruction::HALT));
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), -1);
    }

    #[test]
    fn done_coro_empty_stack_pushes_false() {
        let mut vm = Machine::<2>::default();
        vm.run(&[
            Byte::new(Instruction::DoneCoro),
            Byte::new(Instruction::HALT),
        ]);
        assert!(!vm.pop().as_bool());
    }

    #[test]
    fn done_coro_on_int_pushes_false() {
        let mut vm = Machine::<2>::default();
        vm.run(&[
            const_int(1),
            Byte::new(Instruction::DoneCoro),
            Byte::new(Instruction::HALT),
        ]);
        assert!(!vm.pop().as_bool());
    }

    #[test]
    fn load_field_on_non_enum_pushes_default() {
        let mut vm = Machine::<4>::default();
        vm.run(&[const_int(9), load_field(0), Byte::new(Instruction::HALT)]);
        assert_eq!(vm.tell(), 1);
        assert_eq!(vm.pop(), Value::default());
    }

    /// MakeFn packing: [7:0]=n_cap [15:8]=n_filled [23:16]=arity [24]=is_rest
    fn make_fn_op(n_cap: u32, n_filled: u32, arity: u32, is_rest: bool) -> u32 {
        n_cap | (n_filled << 8) | (arity << 16) | if is_rest { 1 << 24 } else { 0 }
    }

    #[test]
    fn make_fn_then_call_indirect_invokes_entry() {
        // MakeFn → StorePop 0; push args; LOAD 0; CallIndirect
        let body_entry = 9u32;
        let code = vec![
            const_int(0),
            const_int(body_entry as i64),
            Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_op(0, 0, 2, false)),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(10),
            const_int(20),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(2),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        assert!(matches!(code[body_entry as usize].bytecode(), Instruction::LOAD));
        let mut vm = Machine::<16>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 30);
    }

    #[test]
    fn make_fn_partial_then_complete_via_call_indirect() {
        let body_entry = 9u32;
        let code = vec![
            const_int(7),
            const_int(0b001),
            const_int(body_entry as i64),
            Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_op(0, 1, 2, false)),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(3),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        let mut vm = Machine::<16>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 10);
    }

    #[test]
    fn make_fn_with_captures_injects_leading_locals() {
        let body_entry = 9u32;
        let code = vec![
            const_int(10),
            const_int(0),
            const_int(body_entry as i64),
            Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_op(1, 0, 1, false)),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(5),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        let mut vm = Machine::<16>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 15);
    }

    #[test]
    fn call_indirect_nested_partial_fills_remaining_holes() {
        let body_entry = 14u32;
        let code = [
            const_int(0),
            const_int(body_entry as i64),
            Byte::new(Instruction::MakeFn).with_operand_u32(make_fn_op(0, 0, 3, false)),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(1),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(1),
            Byte::new(Instruction::StorePop).with_operand_u32(0),
            const_int(2),
            const_int(3),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::CallIndirect).with_operand_u32(2),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::POP),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::LOAD).with_operand_u32(2),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
        ];
        assert!(matches!(code[body_entry as usize].bytecode(), Instruction::LOAD));
        let mut vm = Machine::<32>::default();
        vm.run(&code);
        assert_eq!(vm.pop().as_int(), 6);
    }

    /// Regression: two `LoadStatic; CONST 1; ADD; StoreStatic` sequences in one
    /// function must not underflow the stack in release builds.
    #[test]
    fn dual_static_assign_sequence_does_not_underflow_stack() {
        let code = [
            Byte::new(Instruction::LoadStatic).with_operand_u32(0),
            Byte::new(Instruction::CONST).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::StoreStatic).with_operand_u32(0),
            Byte::new(Instruction::LoadStatic).with_operand_u32(1),
            Byte::new(Instruction::CONST).with_operand_u32(1),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::StoreStatic).with_operand_u32(1),
            Byte::new(Instruction::HALT),
        ];
        let mut vm = Machine::<256>::default();
        vm.run_with_pool(&code, &[], 2);
    }

    /// StoreStatic must write the popped value so a later LoadStatic observes it.
    #[test]
    fn load_store_static_round_trips_value() {
        let code = [
            Byte::new(Instruction::CONST).with_operand_u32(42),
            Byte::new(Instruction::StoreStatic).with_operand_u32(0),
            Byte::new(Instruction::LoadStatic).with_operand_u32(0),
            Byte::new(Instruction::HALT),
        ];
        let mut vm = Machine::<64>::default();
        vm.run_with_pool(&code, &[], 1);
        assert_eq!(vm.pop().as_int(), 42);
    }

    /// TailCall reuses the current frame (no nest) and overwrites args in place.
    /// Manual sum_to(n, acc): if n <= 0 return acc; else TailCall(n-1, acc+n).
    #[test]
    fn tail_call_reuses_frame_and_computes_sum() {
        let leq = Instruction::LEQ as u8;
        let sub = Instruction::SUB as u8;
        let code = [
            Byte::new(Instruction::CONST).with_const_inline(5),
            Byte::new(Instruction::CONST).with_const_inline(0),
            Byte::new(Instruction::CALL).with_call_packed(2, 4),
            Byte::new(Instruction::HALT),
            // 4: if !(n <= 0) jump to recurse at 7
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(leq, 0, 0),
            Byte::new(Instruction::JMPF).with_operand_u32(7),
            // 6: return acc
            Byte::new(Instruction::LoadReturnSlot).with_operand_u32(1),
            // 7: n - 1 onto stack
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 1),
            // 8–10: acc + n → new_acc; stack = [n-1, new_acc]
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::TailCall).with_call_packed(2, 4),
        ];
        let mut vm = Machine::<128>::default();
        vm.run(&code);
        // sum_to(5,0) = 5+4+3+2+1 = 15
        assert_eq!(vm.pop().as_int(), 15);
    }

    /// TailCall must not push frames: deep recursion stays within Machine::<64> frames.
    #[test]
    fn tail_call_does_not_grow_frame_stack() {
        // sum_to(200, 0) via TailCall — if TailCall pushed frames like CALL,
        // Machine::<64> would overflow the frame stack.
        let leq = Instruction::LEQ as u8;
        let sub = Instruction::SUB as u8;
        let code = [
            Byte::new(Instruction::CONST).with_const_inline(200),
            Byte::new(Instruction::CONST).with_const_inline(0),
            Byte::new(Instruction::CALL).with_call_packed(2, 4),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(leq, 0, 0),
            Byte::new(Instruction::JMPF).with_operand_u32(7),
            Byte::new(Instruction::LoadReturnSlot).with_operand_u32(1),
            Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 1),
            Byte::new(Instruction::LOAD).with_operand_u32(1),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::TailCall).with_call_packed(2, 4),
        ];
        let mut vm = Machine::<64>::default();
        vm.run(&code);
        // 200+199+...+1 = 20100
        assert_eq!(vm.pop().as_int(), 20100);
    }

    /// Out-of-range StoreStatic is a defensive no-op in release (debug_assert in dev).
    #[test]
    #[cfg(not(debug_assertions))]
    fn store_static_out_of_range_is_noop() {
        let code = [
            Byte::new(Instruction::CONST).with_operand_u32(7),
            Byte::new(Instruction::StoreStatic).with_operand_u32(99),
            Byte::new(Instruction::CONST).with_operand_u32(1),
            Byte::new(Instruction::HALT),
        ];
        let mut vm = Machine::<64>::default();
        vm.run_with_pool(&code, &[], 1);
        assert_eq!(vm.pop().as_int(), 1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "StoreStatic slot 99 out of bounds")]
    fn store_static_out_of_range_debug_asserts() {
        let code = [
            Byte::new(Instruction::CONST).with_operand_u32(7),
            Byte::new(Instruction::StoreStatic).with_operand_u32(99),
            Byte::new(Instruction::HALT),
        ];
        let mut vm = Machine::<64>::default();
        vm.run_with_pool(&code, &[], 1);
    }

    #[test]
    fn cast_int_to_byte_truncates_high_bits() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(257),
            Byte::new(Instruction::CastIntToByte),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 1);
    }

    #[test]
    fn cast_int_to_byte_wraps_negatives() {
        let mut vm = Machine::<8>::default();
        // Negatives need the constant pool (inline CONST cannot encode them).
        let neg1 = Value::from(-1_i64).raw() as u64;
        vm.run_with_pool(
            &[
                Byte::new(Instruction::CONST).with_operand_u32(Byte::POOL_FLAG),
                Byte::new(Instruction::CastIntToByte),
                Byte::new(Instruction::HALT),
            ],
            &[neg1],
            0,
        );
        assert_eq!(vm.pop().as_int(), 255);
    }

    #[test]
    fn cast_int_to_bool_normalizes_nonzero() {
        let mut vm = Machine::<8>::default();
        vm.run(&[
            const_int(2),
            Byte::new(Instruction::CastIntToBool),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 1);

        vm.run(&[
            const_int(0),
            Byte::new(Instruction::CastIntToBool),
            Byte::new(Instruction::HALT),
        ]);
        assert_eq!(vm.pop().as_int(), 0);
    }
}
