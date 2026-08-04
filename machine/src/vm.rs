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
    ProgramDebug, Value, byte_to_position, likely, promise, unlikely,
};

use crate::{
    CStructLayout, CoroState, Frame, Heap, Member, ObjArray, ObjBoxed, ObjCoroutine, ObjEnum,
    ObjFn, ObjInstance, ObjPolyFn, ObjString, ObjTuple, Object, RefCoroutine, Stack,
};
#[cfg(any(test, feature = "debugger"))]
use crate::{DebugController, StopReason};
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

/// Parked HostInvoke waiting on IO readiness (`await_readable` / `await_writable`).
struct PendingIoWait {
    request: crate::io::IoParkRequest,
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
    program_strings: Vec<String>,
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
    /// Set when `await_*` parks until fd readiness (CPU help-steals meanwhile).
    pending_io: Option<PendingIoWait>,
    /// Set when a language-level `panic` aborts the VM.
    panicked: bool,
    /// Global static slots (`LoadStatic` / `StoreStatic`).
    statics: Vec<Value>,
    /// Debug line table (parallel to archived bytecode indices).
    program_debug: ProgramDebug,
    /// Cached `(file_index, line)` per PC for debug stepping (built from `program_debug`).
    pc_lines: Vec<Option<(u32, u32)>>,
    #[cfg(any(test, feature = "debugger"))]
    /// Optional debug controller; when set, `execute` may pause at stops.
    debug: Option<Box<DebugController>>,
    #[cfg(any(test, feature = "debugger"))]
    /// Set when `execute` pauses for the debugger (alongside `pending_ffi`).
    pending_debug_stop: Option<StopReason>,
    /// Shared program image for OS thread workers (`spawn`).
    thread_program: Option<std::sync::Arc<crate::thread::ThreadProgram>>,
    /// Optional shared stdout capture for worker threads.
    shared_print: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
    /// Undetached spawns owned by this VM (joined at end of `run_with_pool`).
    live_threads: crate::thread::LiveThreadRegistry,
    /// Shared concurrent OS-worker budget for this root VM (and its workers).
    worker_cap: std::sync::Arc<crate::thread::WorkerCap>,
    /// Work-stealing pool sized by [`Self::worker_cap`].
    reactor: std::sync::Arc<crate::reactor::Reactor>,
    /// IO readiness reactor (sync adapters + async waiters).
    io_reactor: std::sync::Arc<crate::io_reactor::IoReactor>,
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        let worker_cap = crate::thread::WorkerCap::new();
        let reactor = crate::reactor::Reactor::new(worker_cap.max());
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
            program_strings: Vec::new(),
            nested_depth: 0,
            nested_frame_depths: Vec::new(),
            nested_return: None,
            pending_ffi: None,
            pending_io: None,
            panicked: false,
            statics: Vec::new(),
            program_debug: ProgramDebug::default(),
            pc_lines: Vec::new(),
            #[cfg(any(test, feature = "debugger"))]
            debug: None,
            #[cfg(any(test, feature = "debugger"))]
            pending_debug_stop: None,
            thread_program: None,
            shared_print: None,
            live_threads: crate::thread::new_live_thread_registry(),
            worker_cap,
            reactor,
            io_reactor: crate::io_reactor::IoReactor::new(),
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
        self.rebuild_pc_line_cache();
    }

    /// Attach a debug controller (enables stop checks in `execute`).
    #[cfg(any(test, feature = "debugger"))]
    pub fn attach_debug(&mut self, controller: DebugController) {
        self.debug = Some(Box::new(controller));
        self.pending_debug_stop = None;
        if self.pc_lines.is_empty() {
            self.rebuild_pc_line_cache();
        }
    }

    /// Borrow the attached debug controller, if any.
    #[cfg(any(test, feature = "debugger"))]
    pub fn debug_controller_mut(&mut self) -> Option<&mut DebugController> {
        self.debug.as_deref_mut()
    }

    #[cfg(any(test, feature = "debugger"))]
    pub fn debug_controller(&self) -> Option<&DebugController> {
        self.debug.as_deref()
    }

    fn rebuild_pc_line_cache(&mut self) {
        use std::collections::HashMap;
        let mut texts: HashMap<u32, String> = HashMap::new();
        self.pc_lines.clear();
        self.pc_lines.reserve(self.program_debug.debug_locs.len());
        for loc in &self.program_debug.debug_locs {
            if !loc.is_known() {
                self.pc_lines.push(None);
                continue;
            }
            let text = texts.entry(loc.file).or_insert_with(|| {
                let path = self
                    .program_debug
                    .source_files
                    .get(loc.file as usize)
                    .map(|p| self.resolve_source_path(p))
                    .unwrap_or_default();
                std::fs::read_to_string(path).unwrap_or_default()
            });
            if text.is_empty() {
                self.pc_lines.push(None);
                continue;
            }
            let pos = byte_to_position(text, loc.start_byte as usize);
            self.pc_lines.push(Some((loc.file, pos.line)));
        }
    }

    /// Resolve PC → `(path, line, column)` when debug locs are known.
    pub fn resolve_pc_location(&self, pc: usize) -> Option<(String, u32, u32)> {
        let loc = self.program_debug.debug_locs.get(pc)?;
        if !loc.is_known() {
            return None;
        }
        let path = self.program_debug.source_files.get(loc.file as usize)?;
        let resolved = self.resolve_source_path(path);
        let text = std::fs::read_to_string(&resolved).ok()?;
        let pos = byte_to_position(&text, loc.start_byte as usize);
        Some((resolved.display().to_string(), pos.line, pos.column))
    }

    pub fn debug_ip(&self) -> usize {
        if self.frames.is_empty() {
            return 0;
        }
        self.frames.get().tell()
    }

    pub fn debug_frame_depth(&self) -> usize {
        self.frames.len()
    }

    pub fn debug_frame_sp(&self, frame_idx: usize) -> Option<usize> {
        if frame_idx >= self.frames.len() {
            return None;
        }
        Some(self.frames[frame_idx].get())
    }

    pub fn debug_frame_ip(&self, frame_idx: usize) -> Option<usize> {
        if frame_idx >= self.frames.len() {
            return None;
        }
        Some(self.frames[frame_idx].tell())
    }

    /// Read local/operand slot `slot` relative to frame base (`frame.sp + slot`).
    pub fn debug_slot(&self, frame_idx: usize, slot: usize) -> Option<Value> {
        let base = self.debug_frame_sp(frame_idx)?;
        let idx = base + slot;
        if idx >= self.stack.tell() && idx >= 8192 {
            return None;
        }
        // Allow reading within the stack buffer even past cursor for allocated locals.
        if idx >= 8192 {
            return None;
        }
        Some(self.stack[idx])
    }

    pub fn debug_format_value(&self, v: Value) -> String {
        Self::stringify_value(&self.heap, v)
    }

    pub fn program_debug(&self) -> &ProgramDebug {
        &self.program_debug
    }

    /// Cached `(file_index, line)` for a PC, if known.
    pub fn debug_pc_line(&self, pc: usize) -> Option<(u32, u32)> {
        self.pc_lines.get(pc).copied().flatten()
    }

    /// Reset execution state for a fresh `run` (keeps natives / debug / program_debug).
    #[cfg(any(test, feature = "debugger"))]
    pub fn debug_reset(&mut self) {
        self.stack = Stack::new();
        self.frames = ArrayVec::default();
        self.frames.consume();
        self.panicked = false;
        self.pending_ffi = None;
        self.pending_io = None;
        self.pending_debug_stop = None;
        self.nested_depth = 0;
        self.nested_frame_depths.clear();
        self.nested_return = None;
        self.resume_stack.clear();
        self.statics.clear();
        self.alloc_counter = 0;
        if let Some(dbg) = self.debug.as_mut() {
            dbg.clear_step();
            dbg.clear_skip_bp();
        }
    }

    #[cfg(any(test, feature = "debugger"))]
    fn debug_check_stop_at(&mut self, ip: usize) -> Option<StopReason> {
        let depth = self.frames.len();
        let loc = self.pc_lines.get(ip).copied().flatten();
        self.debug.as_mut()?.check_stop(ip, depth, loc)
    }

    /// Run until the next debug stop, halt, or panic. Auto-resumes FFI pauses.
    #[cfg(any(test, feature = "debugger"))]
    pub fn debug_run_until(
        &mut self,
        code: &[Byte],
        constants: &[u64],
        strings: &[String],
        static_slots: u32,
        start_ip: usize,
    ) -> StopReason {
        if code.is_empty() {
            return StopReason::Halt;
        }
        if self.statics.len() != static_slots as usize {
            self.statics = vec![Value::default(); static_slots as usize];
        }
        if self.program_code.is_empty() {
            self.program_code = unsafe {
                std::slice::from_raw_parts(code.as_ptr().cast::<RawByte>(), code.len()).to_vec()
            };
            self.program_constants = constants.to_vec();
            self.program_strings = strings.to_vec();
            self.sync_thread_program_from_current();
        }
        let mut ip = start_ip;
        loop {
            self.pending_debug_stop = None;
            let paused = self.execute(code, constants, ip);
            if let Some(pending) = self.pending_ffi.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_ffi_invoke(pending);
                ip = resume_ip;
                continue;
            }
            if let Some(pending) = self.pending_io.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_io_wait(pending);
                ip = resume_ip;
                continue;
            }
            if let Some(reason) = self.pending_debug_stop.take() {
                return reason;
            }
            if self.panicked {
                return StopReason::Panic;
            }
            if !paused {
                return StopReason::Halt;
            }
            return StopReason::Halt;
        }
    }

    /// Like [`debug_run_until`] for compiler-owned [`RawByte`] buffers.
    #[cfg(any(test, feature = "debugger"))]
    pub fn debug_run_until_raw(
        &mut self,
        code: &[RawByte],
        constants: &[u64],
        strings: &[String],
        static_slots: u32,
        start_ip: usize,
    ) -> StopReason {
        let code: &[Byte] = unsafe { std::slice::from_raw_parts(code.as_ptr().cast(), code.len()) };
        self.debug_run_until(code, constants, strings, static_slots, start_ip)
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
        Some(format!("{}:{}:{}", path, pos.line, pos.column))
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
        if v.raw().is_null() {
            // `Value::default()` / unit / false-ish null pointer.
            return "0".into();
        }
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
            Some(_) | None => v.as_int().to_string(),
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
        let mut roots = heap.take_gc_roots();
        // Only live operand-stack slots (not the full 8192 buffer).
        for v in stack.as_slice() {
            let addr = v.raw() as u64;
            if addr != 0 && heap.find_object_by_addr(addr).is_some() {
                roots.push(addr);
            }
        }

        for ctx in resume_stack {
            roots.push(ctx.coro.as_ptr() as u64);
        }

        // Conservatively root values held in suspended coroutine stacks.
        for obj in heap.into_iter() {
            if let Object::Coroutine(gc) = obj {
                roots.push(gc.as_ptr() as u64);
                for v in &gc.as_ref().saved_stack {
                    let addr = v.raw() as u64;
                    if addr != 0 && heap.find_object_by_addr(addr).is_some() {
                        roots.push(addr);
                    }
                }
                if let Some(delegate) = &gc.as_ref().yield_from {
                    roots.push(delegate.as_ptr() as u64);
                }
            }
        }

        heap.trace(&roots);

        let (mut gray, mut root_objects) = heap.take_gc_worklists();
        let mut current = heap.head_for_lookup();
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

        heap.restore_gc_worklists(gray, root_objects);
        heap.restore_gc_roots(roots);
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
        if addr == 0 {
            return;
        }
        if let Some(child) = Self::find_object_by_addr(heap, addr) {
            child.mark(gray);
        }
    }

    /// Intern `data`, push the GC pointer, then maybe collect.
    ///
    /// The intern table is a cache, not a GC root — unmarked interned strings
    /// are swept. The new object must be on the operand stack before
    /// [`Self::gc_collect`] so it survives the cycle.
    fn push_interned_string(&mut self, data: String) {
        let gc_string = self.heap.intern(data);
        self.stack
            .push(Value::from(gc_string.as_ptr() as *mut u8 as u64));
        self.alloc_counter += 1;
        if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
            Self::gc_collect(
                &mut self.heap,
                &self.stack,
                &self.resume_stack,
                &mut self.alloc_counter,
            );
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
        if let Some(out) = self.output.as_mut() {
            crate::io::set_output_redirect(Some(out.as_mut() as *mut (dyn IoWrite + Send)));
        }
        prev
    }

    /// Reset the output sink back to stdout. Returns the previous
    /// sink so the caller can recover it (useful in tests that
    /// want to scope the redirection).
    pub fn restore_output(&mut self) -> Option<OutputSink> {
        crate::io::set_output_redirect(None);
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
        self.shared_print = Some(buf.clone());
        crate::io::set_shared_print_redirect(Some(buf));
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

    /// Share the root VM's worker-thread budget with nested workers.
    pub fn set_worker_cap(&mut self, cap: std::sync::Arc<crate::thread::WorkerCap>) {
        self.worker_cap = cap;
    }

    pub fn worker_cap(&self) -> &std::sync::Arc<crate::thread::WorkerCap> {
        &self.worker_cap
    }

    /// Share the root VM's work-stealing reactor with nested workers.
    pub fn set_reactor(&mut self, reactor: std::sync::Arc<crate::reactor::Reactor>) {
        self.reactor = reactor;
    }

    pub fn reactor(&self) -> &std::sync::Arc<crate::reactor::Reactor> {
        &self.reactor
    }

    /// Share the root VM's IO reactor with nested workers.
    pub fn set_io_reactor(&mut self, io: std::sync::Arc<crate::io_reactor::IoReactor>) {
        self.io_reactor = io;
    }

    pub fn io_reactor(&self) -> &std::sync::Arc<crate::io_reactor::IoReactor> {
        &self.io_reactor
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
            worker_cap: std::sync::Arc::clone(&self.worker_cap),
            reactor: std::sync::Arc::clone(&self.reactor),
            io_reactor: std::sync::Arc::clone(&self.io_reactor),
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
            strings: std::sync::Arc::new(self.program_strings.clone()),
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
        // Coroutine resume bookkeeping is cold for ordinary calls (fib).
        if unlikely(!self.resume_stack.is_empty())
            && let Some(ctx) = self.resume_stack.last()
            && self.frames.len() <= ctx.frame_depth
        {
            self.with_coroutine_mut(ctx.coro.as_ptr() as u64, |coro| {
                coro.state = CoroState::Done;
                coro.saved_stack.clear();
                coro.saved_frames.clear();
                coro.yield_from = None;
            });
            self.resume_stack.pop();
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
            && matches!(
                code[*ip].bytecode(),
                Instruction::STORE | Instruction::StorePop
            )
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
    pub fn load_program(&mut self, code: &[RawByte], constants: &[u64], strings: &[String]) {
        self.program_code = code.to_vec();
        self.program_constants = constants.to_vec();
        self.program_strings = strings.to_vec();
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
        self.run_with_pool(code, &[], &[], 0);
    }

    /// Run bytecode with an optional constant pool for wide immediates.
    pub fn run_with_pool(
        &mut self,
        code: &[Byte],
        constants: &[u64],
        strings: &[String],
        static_slots: u32,
    ) {
        if code.is_empty() {
            return;
        }
        self.statics = vec![Value::default(); static_slots as usize];
        self.program_code = unsafe {
            std::slice::from_raw_parts(code.as_ptr().cast::<RawByte>(), code.len()).to_vec()
        };
        self.program_constants = constants.to_vec();
        self.program_strings = strings.to_vec();
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
            if let Some(pending) = self.pending_io.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_io_wait(pending);
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

    fn finish_pending_io_wait(&mut self, pending: PendingIoWait) {
        self.frames.get_mut().set(pending.resume_sp);
        let req = pending.request;
        let wait = crate::thread::host_io_wait(req.fd, req.interest, req.timeout);
        let v = crate::io::as_result_unit(&mut self.heap, wait);
        self.stack.push(v);
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
        // Borrow constants without cloning; stable while `program_constants` is not resized.
        let constants: &[u64] = unsafe {
            std::slice::from_raw_parts(
                self.program_constants.as_ptr(),
                self.program_constants.len(),
            )
        };
        let mut ip = offset as usize;
        loop {
            let paused = self.execute(code, constants, ip);
            if let Some(pending) = self.pending_ffi.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_ffi_invoke(pending);
                ip = resume_ip;
                continue;
            }
            if let Some(pending) = self.pending_io.take() {
                let resume_ip = pending.resume_ip;
                self.finish_pending_io_wait(pending);
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
        // Nested FFI/host calls are rare; keep the hot RETURN path branch-free.
        if unlikely(self.nested_depth > 0) {
            let nested_target = self.nested_frame_depths.last().copied().unwrap_or(0);
            if self.frames.len() == nested_target {
                self.nested_return = Some(ret_val);
                return true;
            }
        }
        false
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
    pub fn run_raw(
        &mut self,
        code: &[RawByte],
        constants: &[u64],
        strings: &[String],
        static_slots: u32,
    ) {
        let code: &[Byte] = unsafe { std::slice::from_raw_parts(code.as_ptr().cast(), code.len()) };
        self.run_with_pool(code, constants, strings, static_slots);
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

        let mut ip: usize = start_ip;
        let mut sp = self.frames.get_mut().get();

        while ip < code.len() {
            #[cfg(any(test, feature = "debugger"))]
            if unlikely(self.debug.is_some())
                && let Some(reason) = self.debug_check_stop_at(ip)
            {
                self.frames.get_mut().seek(ip);
                self.frames.get_mut().set(sp);
                self.pending_debug_stop = Some(reason);
                return true;
            }

            #[cfg(any(test, feature = "vm_profile"))]
            VM_DISPATCH_COUNT.with(|c| c.fetch_add(1, Ordering::Relaxed));

            // SAFETY: loop condition guarantees `ip < code.len()`.
            promise!(ip < code.len());
            let opcode = unsafe { code.get_unchecked(ip) };
            ip += 1;

            let bc = opcode.bytecode();
            // Release-only optimizer hint: must track the LAST `Instruction`
            // variant. A stale ceiling (e.g. YieldFromCoro) makes later opcodes
            // (`StoreIndex`, `DoneCoro`, `ArrayPush`, …) UB via assert_unchecked.
            #[cfg(not(debug_assertions))]
            promise!(*bc as u8 <= Instruction::BinSlotSlotStore as u8);

            match bc {
                Instruction::POP => {
                    self.stack.pop();
                }
                Instruction::DUPLICATE => {
                    self.stack.duplicate();
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
                    // Pop TOS into each listed slot (packed n=1..=3, or wide n=0).
                    // Extend the cursor when a slot is newly allocated, but never
                    // shrink past higher locals — locals and the operand stack share memory.
                    let count = opcode.load_store_count();
                    for i in 0..count {
                        let slot = sp + opcode.load_store_slot_at(i) as usize;
                        let val = self.stack.pop();
                        self.stack[slot] = val;
                        let tell = self.stack.tell();
                        if tell < slot + 1 {
                            self.stack.seek(slot + 1);
                        }
                    }
                }
                Instruction::LOAD => {
                    let count = opcode.load_store_count();
                    for i in 0..count {
                        let slot = opcode.load_store_slot_at(i) as usize;
                        promise!(sp + slot < 8192);
                        self.stack.push(self.stack[sp + slot]);
                    }
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

                        self.push_interned_string(message);
                    }
                }
                Instruction::STRINGIFY => {
                    // Shared primitive conversion for Show thunks / `%v`.
                    // Accepts a boxed value (preferred), a heap string, or a
                    // raw immediate (treated as int).
                    let v = self.stack.pop();
                    let text = Self::stringify_value(&self.heap, v);
                    self.push_interned_string(text);
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
                    let callee_sp = self.stack.tell() - arity;
                    // Direct calls dominate; avoid the indirect `target == 0`
                    // return-ip adjustment on that path.
                    if likely(target != 0) {
                        self.frames.rewrite_top_and_push(
                            |caller| caller.seek(ip),
                            |frame| frame.set(callee_sp),
                        );
                        sp = callee_sp;
                        ip = target;
                    } else {
                        self.frames.rewrite_top_and_push(
                            |caller| caller.seek(ip + 1),
                            |frame| frame.set(callee_sp),
                        );
                        sp = callee_sp;
                    }
                }
                Instruction::TailCall => {
                    let (arity, target) = opcode.call_parts();
                    let callee_sp = self.frames.get().get();
                    let src = self.stack.tell() - arity;
                    // Args sit at TOS; frame base is at or below them.
                    self.stack.copy_slots(callee_sp, src, arity);
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                // Fused `LOAD slot; CONST imm; <binop>` — compute in place
                // (same shape as `BinSlotSlot`) to avoid two temp pushes.
                Instruction::BinSlotImm => {
                    let (op, slot, imm) = opcode.bin_slot_imm_parts();
                    promise!(sp + slot < 8192);
                    let lhs = self.stack[sp + slot];
                    let rhs = Value::from(imm);
                    let result = match Instruction::from(op) {
                        Instruction::ADD => Value::from(lhs.as_int() + imm),
                        Instruction::SUB => Value::from(lhs.as_int() - imm),
                        Instruction::MUL => Value::from(lhs.as_int() * imm),
                        Instruction::DIV => Value::from(lhs.as_int() / imm),
                        Instruction::MOD => Value::from(lhs.as_int() % imm),
                        Instruction::LE => Value::from((lhs.raw() < rhs.raw()) as i64),
                        Instruction::LEQ => Value::from((lhs.raw() <= rhs.raw()) as i64),
                        Instruction::GT => Value::from((lhs.raw() > rhs.raw()) as i64),
                        Instruction::GEQ => Value::from((lhs.raw() >= rhs.raw()) as i64),
                        Instruction::EQ => Value::from((lhs.raw() == rhs.raw()) as i64),
                        Instruction::NEQ => Value::from((lhs.raw() != rhs.raw()) as i64),
                        Instruction::Pow => {
                            let exp = imm.max(0) as u32;
                            Value::from(lhs.as_int().pow(exp))
                        }
                        Instruction::BITAND => Value::from(lhs.as_int() & imm),
                        Instruction::BITOR => Value::from(lhs.as_int() | imm),
                        Instruction::SHL => Value::from(lhs.as_int() << imm),
                        Instruction::SHR => Value::from(lhs.as_int() >> imm),
                        Instruction::XOR => Value::from(lhs.as_int() ^ imm),
                        Instruction::AND => Value::from(lhs.as_bool() && rhs.as_bool()),
                        Instruction::OR => Value::from(lhs.as_bool() || rhs.as_bool()),
                        _ => Value::default(),
                    };
                    self.stack.push(result);
                }
                // Fused `<cmp|cond>; JMPF target`.
                Instruction::CmpJmpf => {
                    let (op, t) = opcode.cmp_jmpf_parts();
                    let target = if opcode.cmp_jmpf_is_pool() {
                        promise!(t < constants.len());
                        unsafe { *constants.get_unchecked(t) as usize }
                    } else {
                        t
                    };
                    let tos = self.stack.tell();
                    promise!(tos >= 2);
                    let rhs = self.stack[tos - 1];
                    let lhs = self.stack[tos - 2];
                    self.stack.seek(tos - 2);
                    let taken = match Instruction::from(op) {
                        Instruction::LE => lhs.raw() < rhs.raw(),
                        Instruction::LEQ => lhs.raw() <= rhs.raw(),
                        Instruction::GT => lhs.raw() > rhs.raw(),
                        Instruction::GEQ => lhs.raw() >= rhs.raw(),
                        Instruction::EQ => lhs.raw() == rhs.raw(),
                        Instruction::NEQ => lhs.raw() != rhs.raw(),
                        Instruction::LEF => lhs.as_float() < rhs.as_float(),
                        Instruction::LEQF => lhs.as_float() <= rhs.as_float(),
                        Instruction::GTF => lhs.as_float() > rhs.as_float(),
                        Instruction::GEQF => lhs.as_float() >= rhs.as_float(),
                        Instruction::AND => lhs.as_bool() && rhs.as_bool(),
                        Instruction::OR => lhs.as_bool() || rhs.as_bool(),
                        Instruction::BITAND => Value::from(lhs.as_int() & rhs.as_int()).as_bool(),
                        Instruction::BITOR => Value::from(lhs.as_int() | rhs.as_int()).as_bool(),
                        Instruction::XOR => Value::from(lhs.as_int() ^ rhs.as_int()).as_bool(),
                        _ => false,
                    };
                    if !taken {
                        ip = target;
                    }
                }
                // Fused `LOAD slot; CONST imm; <cond>; JMPF` without stack traffic.
                Instruction::BinSlotImmJmpf => {
                    let (op, slot, pool_idx) = opcode.bin_slot_imm_jmpf_parts();
                    promise!(pool_idx < constants.len());
                    let packed = unsafe { *constants.get_unchecked(pool_idx) };
                    let imm = packed as u32 as i32 as i64;
                    let target = (packed >> 32) as usize;
                    promise!(sp + slot < 8192);
                    let lhs = self.stack[sp + slot];
                    let rhs = Value::from(imm);
                    let taken = match Instruction::from(op) {
                        Instruction::LE => lhs.raw() < rhs.raw(),
                        Instruction::LEQ => lhs.raw() <= rhs.raw(),
                        Instruction::GT => lhs.raw() > rhs.raw(),
                        Instruction::GEQ => lhs.raw() >= rhs.raw(),
                        Instruction::EQ => lhs.raw() == rhs.raw(),
                        Instruction::NEQ => lhs.raw() != rhs.raw(),
                        Instruction::LEF => lhs.as_float() < rhs.as_float(),
                        Instruction::LEQF => lhs.as_float() <= rhs.as_float(),
                        Instruction::GTF => lhs.as_float() > rhs.as_float(),
                        Instruction::GEQF => lhs.as_float() >= rhs.as_float(),
                        Instruction::AND => lhs.as_bool() && rhs.as_bool(),
                        Instruction::OR => lhs.as_bool() || rhs.as_bool(),
                        Instruction::BITAND => Value::from(lhs.as_int() & imm).as_bool(),
                        Instruction::BITOR => Value::from(lhs.as_int() | imm).as_bool(),
                        Instruction::XOR => Value::from(lhs.as_int() ^ imm).as_bool(),
                        _ => false,
                    };
                    if !taken {
                        ip = target;
                    }
                }
                Instruction::LogNotJmpf => {
                    let t = opcode.log_not_jmpf_target();
                    let target = if opcode.log_not_jmpf_is_pool() {
                        promise!(t < constants.len());
                        unsafe { *constants.get_unchecked(t) as usize }
                    } else {
                        t
                    };
                    let val = self.stack.pop();
                    if val.as_int() != 0 {
                        ip = target;
                    }
                }
                // Fused `BinSlotSlot; JMPF` — pool packs (target<<32)|b.
                Instruction::BinSlotSlotJmpf => {
                    let (op, a, pool_idx) = opcode.bin_slot_slot_jmpf_parts();
                    promise!(pool_idx < constants.len());
                    let packed = unsafe { *constants.get_unchecked(pool_idx) };
                    let b = (packed as u32 & 0xFF) as usize;
                    let target = (packed >> 32) as usize;
                    promise!(sp + a < 8192);
                    promise!(sp + b < 8192);
                    let va = self.stack[sp + a];
                    let vb = self.stack[sp + b];
                    let taken = match Instruction::from(op) {
                        Instruction::LE => va.raw() < vb.raw(),
                        Instruction::LEQ => va.raw() <= vb.raw(),
                        Instruction::GT => va.raw() > vb.raw(),
                        Instruction::GEQ => va.raw() >= vb.raw(),
                        Instruction::EQ => va.raw() == vb.raw(),
                        Instruction::NEQ => va.raw() != vb.raw(),
                        Instruction::LEF => va.as_float() < vb.as_float(),
                        Instruction::LEQF => va.as_float() <= vb.as_float(),
                        Instruction::GTF => va.as_float() > vb.as_float(),
                        Instruction::GEQF => va.as_float() >= vb.as_float(),
                        Instruction::AND => va.as_bool() && vb.as_bool(),
                        Instruction::OR => va.as_bool() || vb.as_bool(),
                        Instruction::BITAND => Value::from(va.as_int() & vb.as_int()).as_bool(),
                        Instruction::BITOR => Value::from(va.as_int() | vb.as_int()).as_bool(),
                        Instruction::XOR => Value::from(va.as_int() ^ vb.as_int()).as_bool(),
                        _ => false,
                    };
                    if !taken {
                        ip = target;
                    }
                }
                // Fused `LOAD src; CONST imm; <op>; STORE dest` — pool packs (dest<<32)|imm.
                Instruction::BinSlotImmStore => {
                    let (op, slot, pool_idx) = opcode.bin_slot_imm_store_parts();
                    promise!(pool_idx < constants.len());
                    let packed = unsafe { *constants.get_unchecked(pool_idx) };
                    let imm = packed as u32 as i32 as i64;
                    let dest = (packed >> 32) as usize;
                    promise!(sp + slot < 8192);
                    let lhs = self.stack[sp + slot];
                    let rhs = Value::from(imm);
                    let result = match Instruction::from(op) {
                        Instruction::ADD => Value::from(lhs.as_int() + imm),
                        Instruction::SUB => Value::from(lhs.as_int() - imm),
                        Instruction::MUL => Value::from(lhs.as_int() * imm),
                        Instruction::DIV => Value::from(lhs.as_int() / imm),
                        Instruction::MOD => Value::from(lhs.as_int() % imm),
                        Instruction::LE => Value::from((lhs.raw() < rhs.raw()) as i64),
                        Instruction::LEQ => Value::from((lhs.raw() <= rhs.raw()) as i64),
                        Instruction::GT => Value::from((lhs.raw() > rhs.raw()) as i64),
                        Instruction::GEQ => Value::from((lhs.raw() >= rhs.raw()) as i64),
                        Instruction::EQ => Value::from((lhs.raw() == rhs.raw()) as i64),
                        Instruction::NEQ => Value::from((lhs.raw() != rhs.raw()) as i64),
                        Instruction::Pow => {
                            let exp = imm.max(0) as u32;
                            Value::from(lhs.as_int().pow(exp))
                        }
                        Instruction::BITAND => Value::from(lhs.as_int() & imm),
                        Instruction::BITOR => Value::from(lhs.as_int() | imm),
                        Instruction::SHL => Value::from(lhs.as_int() << imm),
                        Instruction::SHR => Value::from(lhs.as_int() >> imm),
                        Instruction::XOR => Value::from(lhs.as_int() ^ imm),
                        Instruction::AND => Value::from(lhs.as_bool() && rhs.as_bool()),
                        Instruction::OR => Value::from(lhs.as_bool() || rhs.as_bool()),
                        _ => Value::default(),
                    };
                    let dest_idx = sp + dest;
                    self.stack[dest_idx] = result;
                    let tell = self.stack.tell();
                    if tell < dest_idx + 1 {
                        self.stack.seek(dest_idx + 1);
                    }
                }
                // Fused `LOAD a; LOAD b; <op>; STORE dest`.
                Instruction::BinSlotSlotStore => {
                    let (op, a, b, dest) = opcode.bin_slot_slot_store_parts();
                    promise!(sp + a < 8192);
                    promise!(sp + b < 8192);
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
                        Instruction::SHL => Value::from(va.as_int() << vb.as_int()),
                        Instruction::SHR => Value::from(va.as_int() >> vb.as_int()),
                        Instruction::XOR => Value::from(va.as_int() ^ vb.as_int()),
                        Instruction::AND => Value::from(va.as_bool() && vb.as_bool()),
                        Instruction::OR => Value::from(va.as_bool() || vb.as_bool()),
                        Instruction::LE => Value::from((va.raw() < vb.raw()) as i64),
                        Instruction::LEQ => Value::from((va.raw() <= vb.raw()) as i64),
                        Instruction::GT => Value::from((va.raw() > vb.raw()) as i64),
                        Instruction::GEQ => Value::from((va.raw() >= vb.raw()) as i64),
                        Instruction::EQ => Value::from((va.raw() == vb.raw()) as i64),
                        Instruction::NEQ => Value::from((va.raw() != vb.raw()) as i64),
                        _ => Value::default(),
                    };
                    let dest_idx = sp + dest;
                    self.stack[dest_idx] = result;
                    let tell = self.stack.tell();
                    if tell < dest_idx + 1 {
                        self.stack.seek(dest_idx + 1);
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
                    // Compute result without leaving an intermediate TOS;
                    // return unwind reseeks the stack anyway.
                    let tos = self.stack.tell();
                    promise!(tos >= 2);
                    let rhs = self.stack[tos - 1];
                    let lhs = self.stack[tos - 2];
                    let ret_val = match Instruction::from(opcode.bin_return_op()) {
                        Instruction::ADD => Value::from(lhs.as_int() + rhs.as_int()),
                        Instruction::SUB => Value::from(lhs.as_int() - rhs.as_int()),
                        Instruction::MUL => Value::from(lhs.as_int() * rhs.as_int()),
                        Instruction::DIV => Value::from(lhs.as_int() / rhs.as_int()),
                        Instruction::MOD => Value::from(lhs.as_int() % rhs.as_int()),
                        Instruction::ADDF => Value::from(lhs.as_float() + rhs.as_float()),
                        Instruction::SUBF => Value::from(lhs.as_float() - rhs.as_float()),
                        Instruction::MULF => Value::from(lhs.as_float() * rhs.as_float()),
                        Instruction::DIVF => Value::from(lhs.as_float() / rhs.as_float()),
                        Instruction::MODF => Value::from(lhs.as_float() % rhs.as_float()),
                        Instruction::LE => Value::from((lhs.raw() < rhs.raw()) as i64),
                        Instruction::LEQ => Value::from((lhs.raw() <= rhs.raw()) as i64),
                        Instruction::GT => Value::from((lhs.raw() > rhs.raw()) as i64),
                        Instruction::GEQ => Value::from((lhs.raw() >= rhs.raw()) as i64),
                        Instruction::EQ => Value::from((lhs.raw() == rhs.raw()) as i64),
                        Instruction::NEQ => Value::from((lhs.raw() != rhs.raw()) as i64),
                        Instruction::LEF => Value::from((lhs.as_float() < rhs.as_float()) as i64),
                        Instruction::LEQF => Value::from((lhs.as_float() <= rhs.as_float()) as i64),
                        Instruction::GTF => Value::from((lhs.as_float() > rhs.as_float()) as i64),
                        Instruction::GEQF => Value::from((lhs.as_float() >= rhs.as_float()) as i64),
                        Instruction::BITAND => Value::from(lhs.as_int() & rhs.as_int()),
                        Instruction::BITOR => Value::from(lhs.as_int() | rhs.as_int()),
                        Instruction::SHL => Value::from(lhs.as_int() << rhs.as_int()),
                        Instruction::SHR => Value::from(lhs.as_int() >> rhs.as_int()),
                        Instruction::XOR => Value::from(lhs.as_int() ^ rhs.as_int()),
                        Instruction::AND => Value::from(lhs.as_bool() && rhs.as_bool()),
                        Instruction::OR => Value::from(lhs.as_bool() || rhs.as_bool()),
                        Instruction::Pow => {
                            let exp = rhs.as_int().max(0) as u32;
                            Value::from(lhs.as_int().pow(exp))
                        }
                        _ => Value::default(),
                    };
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
                    promise!(sp + a < 8192);
                    promise!(sp + b < 8192);
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
                        Instruction::SHL => Value::from(va.as_int() << vb.as_int()),
                        Instruction::SHR => Value::from(va.as_int() >> vb.as_int()),
                        Instruction::XOR => Value::from(va.as_int() ^ vb.as_int()),
                        Instruction::AND => Value::from(va.as_bool() && vb.as_bool()),
                        Instruction::OR => Value::from(va.as_bool() || vb.as_bool()),
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
                                format!("FFI declare: library at 0x{:x} is not loaded", lib_addr),
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
                    let args: &[Value] = match Self::find_object_by_addr(&self.heap, tuple_addr) {
                        Some(crate::memory::Object::Tuple(gc)) => {
                            let elems = &gc.as_ref().elements;
                            // SAFETY: `tuple_val` keeps the tuple alive until invoke
                            // returns; VM GC runs only after the native finishes.
                            unsafe { std::slice::from_raw_parts(elems.as_ptr(), elems.len()) }
                        }
                        _ => &[],
                    };
                    // Packed LA (and other host natives) allocate via
                    // `heap.alloc` inside the closure; count those so GC
                    // pressure still fires when HostInvoke is the only
                    // allocator on a hot path.
                    let live_before = self.heap.live_object_count();
                    match self.natives.get_by_id(fn_id) {
                        Some(native) => match native.invoke(&mut self.heap, args) {
                            Ok(Some(v)) => self.stack.push(v),
                            Ok(None) => {
                                if let Some(req) = crate::io::take_pending_io_park() {
                                    self.frames.get_mut().set(sp);
                                    self.pending_io = Some(PendingIoWait {
                                        request: req,
                                        resume_ip: ip,
                                        resume_sp: sp,
                                    });
                                    return true;
                                }
                            }
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
                    let allocated = self.heap.live_object_count().saturating_sub(live_before);
                    if allocated > 0 {
                        self.alloc_counter += allocated;
                        if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                    let idx = opcode.operand_u32() as usize;
                    promise!(idx < self.program_strings.len());
                    let value = unsafe { self.program_strings.get_unchecked(idx) }.clone();
                    self.push_interned_string(value);
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
                        let addr = v.raw() as u64;
                        if let Some(o) = Self::find_object_by_addr(&self.heap, addr) {
                            payload.push(Member::Object(o));
                        } else {
                            payload.push(Member::Value(v));
                        }
                    }

                    let obj_enum = ObjEnum { tag, payload };
                    let (object, _) = self.heap.alloc(obj_enum, Object::Enum);

                    self.alloc_counter += 1;
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                    self.stack.push(Value::from(addr));
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
                        Self::gc_collect(
                            &mut self.heap,
                            &self.stack,
                            &self.resume_stack,
                            &mut self.alloc_counter,
                        );
                    }
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        Some(crate::memory::Object::Tuple(gc)) => gc.as_ref().elements.len(),
                        Some(crate::memory::Object::String(gc)) => gc.as_ref().data.len(),
                        Some(crate::memory::Object::Instance(gc)) => {
                            gc.as_ref().iter_fields().count()
                        }
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                // Deprecated discriminant alias of `STORE` (same handler).
                // Compiler never emits StorePop; kept for archived bytecode.
                Instruction::StorePop => {
                    // Deprecated alias of STORE — same packed multi-slot semantics.
                    let count = opcode.load_store_count();
                    for i in 0..count {
                        let slot = sp + opcode.load_store_slot_at(i) as usize;
                        let val = self.stack.pop();
                        self.stack[slot] = val;
                        let tell = self.stack.tell();
                        if tell < slot + 1 {
                            self.stack.seek(slot + 1);
                        }
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        if raw.raw().is_null() {
                            None
                        } else {
                            self.heap.find_object_by_addr(addr).and_then(|o| match o {
                                Object::Fn(gc) => Some(gc),
                                _ => None,
                            })
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
                            if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        if raw.raw().is_null() {
                            (raw.as_int() as usize, Vec::new())
                        } else if let Some(Object::PolyFn(gc)) = self.heap.find_object_by_addr(addr)
                        {
                            let pfn = gc.as_ref();
                            (pfn.entry as usize, pfn.captured_dicts.clone())
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                    let val = self.statics.get(slot).copied().unwrap_or_default();
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
                    let payload = if addr == 0 {
                        Member::Value(v)
                    } else if let Some(obj) = Self::find_object_by_addr(&self.heap, addr) {
                        Member::Object(obj)
                    } else {
                        Member::Value(v)
                    };
                    let boxed = ObjBoxed { tag, payload };
                    let (object, _) = self.heap.alloc(boxed, Object::Boxed);
                    self.alloc_counter += 1;
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        // Already unboxed (e.g. raw enum passed to a Show
                        // thunk that still emits UnboxValue). Pass through.
                        v
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        } else if let Some(obj) = Self::find_object_by_addr(&self.heap, addr) {
                            Some(Member::Object(obj))
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
                    if unlikely(self.alloc_counter > GC_TRIGGER_INTERVAL) {
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
                        if v.raw().is_null() {
                            return (ValueTag::Int, v);
                        }
                        if let Some(obj) = heap.find_object_by_addr(addr) {
                            return match obj {
                                Object::Boxed(gc) => {
                                    let b = gc.as_ref();
                                    let tag = ValueTag::from_u16(b.tag).unwrap_or(ValueTag::Int);
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
                            // Root before any GC (same as FORMAT/STRING).
                            self.push_interned_string(sa + &sb);
                            continue;
                        }
                        _ => {
                            let ai = a_inner.as_int();
                            let bi = b_inner.as_int();
                            let r = match bc_instr {
                                Instruction::DynAdd => ai.wrapping_add(bi),
                                Instruction::DynSub => ai.wrapping_sub(bi),
                                Instruction::DynMul => ai.wrapping_mul(bi),
                                Instruction::DynDiv => {
                                    if bi == 0 {
                                        0
                                    } else {
                                        ai / bi
                                    }
                                }
                                Instruction::DynMod => {
                                    if bi == 0 {
                                        0
                                    } else {
                                        ai % bi
                                    }
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
                        if v.raw().is_null() {
                            return v.as_int();
                        }
                        if let Some(Object::Boxed(gc)) = heap.find_object_by_addr(addr) {
                            return match &gc.as_ref().payload {
                                Member::Value(iv) => iv.as_int(),
                                Member::Object(_) => 0,
                            };
                        }
                        v.as_int()
                    }
                    let kind = opcode.operand_u32() & 0xFF;
                    let b_val = self.stack.pop();
                    let a_val = self.stack.pop();
                    let ai = classify_int_dyn(a_val, &self.heap);
                    let bi = classify_int_dyn(b_val, &self.heap);
                    let result = match kind {
                        0 => ai < bi,  // Le
                        1 => ai <= bi, // Leq
                        2 => ai > bi,  // Gt
                        3 => ai >= bi, // Geq
                        _ => false,
                    };
                    self.stack.push(Value::from(result));
                }
                Instruction::DynEq | Instruction::DynNe => {
                    fn classify_raw_dyn(v: Value, heap: &Heap) -> u64 {
                        let addr = v.raw() as u64;
                        if v.raw().is_null() {
                            return v.raw() as u64;
                        }
                        if let Some(Object::Boxed(gc)) = heap.find_object_by_addr(addr) {
                            return match &gc.as_ref().payload {
                                Member::Value(iv) => iv.raw() as u64,
                                Member::Object(o) => o.addr(),
                            };
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
                    let text = Self::stringify_value(&self.heap, v);
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
#[path = "vm.tests.rs"]
mod tests;
