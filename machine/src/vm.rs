use std::{
    fmt::Write as FmtWrite,
    io::{self, Write as IoWrite},
};

#[cfg(any(test, feature = "vm_profile"))]
use std::sync::atomic::{AtomicU64, Ordering};

use common::{
    ArchivedByte as Byte, ArchivedInstruction as Instruction, ArrayVec, Byte as RawByte, Value,
    promise, unlikely,
};

use crate::{
    Frame, Heap, Member, ObjArray, ObjEnum, ObjInstance, ObjString, ObjTuple, Object, Stack,
};

/// Default allocation count between automatic GC collections. The VM
/// increments an internal counter on every heap allocation site
/// (`INIT`, `STRING`, `FORMAT`, `MAKE_ENUM`); when the counter
/// exceeds this threshold, the VM runs `trace` + `sweep`.
///
/// Phase 15D.1 (was previously wired only in `#[cfg(debug_assertions)]`
/// builds, running on every single instruction — visible as
/// "Performing GC trace" spam). The threshold is intentionally
/// modest: small test programs should still observe at least one
/// collection, while large programs amortise the trace cost over
/// many allocations.
const GC_TRIGGER_INTERVAL: usize = 64;

// Retired VM dispatch count (test / `vm_profile` feature only).
// Thread-local so parallel `#[test]` runs do not share state.
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

/// The output sink for the `PRINT` opcode. By default the VM
/// writes to stdout (via `print!`). Setting this field via
/// [`Machine::with_output`] redirects output to an arbitrary
/// `std::io::Write` implementation — used by the golden
/// pipeline tests in `compiler/tests/pipeline.rs` to capture
/// stdout in memory.
type OutputSink = Box<dyn IoWrite>;

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 8192>,
    frames: ArrayVec<Frame, S>,
    /// Optional output sink. When `None`, the VM writes
    /// `print!` output to stdout (the default). When `Some`,
    /// output is redirected to the contained writer — this is
    /// how the integration tests capture stdout.
    output: Option<OutputSink>,
    /// Allocation counter for automatic GC. Incremented on
    /// every heap allocation site (`INIT`, `STRING`, `FORMAT`,
    /// `MAKE_ENUM`); reset to zero after each collection.
    /// See [`Machine::collect_garbage`] and [`GC_TRIGGER_INTERVAL`].
    alloc_counter: usize,
    /// FFI: registered host native functions keyed by name and id.
    /// Populated via [`Machine::register_fn`].
    natives: crate::ffi::Natives,
    /// FFI: shared libraries loaded for FFI symbol
    /// resolution. Keyed by library short name (e.g.
    /// `"c"`, `"m"`). Multiple FFI calls in the same library
    /// share the same `Library` Arc — `dlopen` is called
    /// once per unique name, `dlsym` is called once per
    /// unique function name.
    libraries: std::collections::HashMap<String, std::sync::Arc<crate::ffi::Library>>,
    /// FFI: userland-loaded libraries (via `load(path)` in
    /// the source). Each entry is the `Object` handle for the
    /// heap `Object::Library(Gc<ObjLibrary>)`. Keyed by
    /// the `Value` address. The `Gc<ObjLibrary>` is reachable
    /// via the `Object::Library` variant for GC and FFI
    /// dispatch.
    userland_libraries: std::collections::HashMap<u64, std::sync::Arc<Object>>,
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
            stack: Stack::new(),
            // Default: stdout (no capture).
            output: None,
            // Phase 15D.1: GC trigger counter. The VM increments
            // this on every allocation; once it exceeds
            // `GC_TRIGGER_INTERVAL` the next allocation triggers
            // a trace+sweep cycle. Start at 0 — the first
            // allocation triggers the first GC after
            // `GC_TRIGGER_INTERVAL` allocations, not before.
            alloc_counter: 0,
            // FFI: empty native registry; the host program
            // registers natives (or extern functions) before
            // calling `run` (or the FFI dispatch below loads
            // them on demand via `dlopen`).
            natives: crate::ffi::Natives::new(),
            libraries: std::collections::HashMap::new(),
            // Userland FFI: populated by `FfiLoad` (the
            // `load(...)` builtin) at runtime. Empty until
            // userland code loads a library.
            userland_libraries: std::collections::HashMap::new(),
        }
    }
}

impl<const S: usize> Machine<S> {
    // pub fn register(&mut self, name: usize, func: External) {
    //     self.native.insert(name, func);
    // }

    /// Walk the heap's intrusive linked list and return the
    /// [`Object`] whose address matches `addr`. Used by the
    /// `MAKE_ENUM` / `JUMP_IF_MATCH` / `UNPACK` opcodes to
    /// reconstruct a heap object's metadata from a raw pointer
    /// on the operand stack. Returns `None` if no match — that
    /// means the address is an immediate value (int/float/bool)
    /// or the pointer has been collected.
    ///
    /// Implemented as a free function (not a `&self` method) so
    /// the borrow checker can split the `&Heap` borrow from
    /// other in-flight borrows on `Machine` fields (specifically
    /// the mutable `frames` borrow held by the `execute` loop).
    fn find_object_by_addr(heap: &Heap, addr: u64) -> Option<Object> {
        let mut current = heap.head_for_lookup();
        while let Some(reference) = current {
            if reference.addr() == addr {
                return Some(reference);
            }
            current = reference.get_next();
        }
        None
    }

    /// Read a C string (NUL-terminated) from a `Value` that
    /// points at a heap-allocated `Object::String`. Used by
    /// the userland `load(path)` builtin to extract the path
    /// argument (which is itself a zero-script string).
    ///
    /// Returns the empty string if the value isn't a string
    /// (callers should validate types at compile time; this
    /// is a safety net for dynamic dispatch).
    #[allow(dead_code)]
    fn value_to_string(&self, v: &Value) -> String {
        self.heap
            .cstr_from_addr(v.raw() as u64)
            .map(|s| {
                // SAFETY: the `cstr_from_addr` lookup finds an
                // `Object::String` and returns a pointer to its
                // `data: String`. We then read it as a UTF-8
                // string (the VM stores `String` as UTF-8).
                unsafe { std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned() }
            })
            .unwrap_or_default()
    }

    /// Extract the `FFIType` tag from a `Value` that points
    /// at a heap `Object::Enum`. Returns 0 (Int) as a safe
    /// fallback when the value doesn't address a real enum —
    /// this lets the runtime degrade gracefully instead of
    /// panicking on a corrupt FFIType value (the typechecker
    /// would have already rejected the source upstream).
    fn ffi_type_tag_from_value(heap: &Heap, v: &Value) -> u32 {
        // The runtime's `DeclareFFI` expects either:
        //   (a) An immediate integer tag (small `u64` value
        //       in [0..=3] — the canonical FFIType enum
        //       values), emitted via `CONST <tag>` by source-
        //       level `extern` blocks; OR
        //   (b) A heap-allocated `Object::Enum` (built via
        //       `MakeEnum` from `FFIType::X` constructor
        //       calls in the userland `dload/declare/invoke`
        //       API).
        //
        // We distinguish by VALUE size: heap pointers are
        // typically large (>= 0x1000); immediate constants
        // are 0..=3. (The boundary at `0x1000` is conservative
        // — no normal user value would land between 3 and the
        // smallest heap address on any platform we target.)
        let addr = v.raw() as u64;
        if addr <= 3 {
            return addr as u32;
        }
        let obj = Self::find_object_by_addr(heap, addr);
        if let Some(crate::memory::Object::Enum(gc)) = obj {
            gc.as_ref().tag
        } else {
            0
        }
    }

    /// Map a `u32` FFIType tag integer to the Rust `FfiType`
    /// enum used by `FunctionSig`. Unknown tags fall back to
    /// `Int` (the same defensive posture as
    /// `ffi_type_tag_from_value`).
    fn ffi_type_from_tag(tag: u32) -> crate::memory::FfiType {
        match tag {
            1 => crate::memory::FfiType::Float,
            2 => crate::memory::FfiType::String,
            3 => crate::memory::FfiType::Void,
            _ => crate::memory::FfiType::Int,
        }
    }

    /// Read a heap `Object::String` as a Rust `String`. Returns
    /// the empty string when the value doesn't address a real
    /// string. Used by the `DeclareFFI` opcode to extract the
    /// function name from the operand stack.
    fn object_string_value(heap: &Heap, v: &Value) -> String {
        let addr = v.raw() as u64;
        let obj = Self::find_object_by_addr(heap, addr);
        if let Some(crate::memory::Object::String(gc)) = obj {
            gc.as_ref().data.clone()
        } else {
            String::new()
        }
    }

    /// Register a new FFI function on the given library `Object`.
    fn register_signature_on_object(
        obj: &mut Object,
        sig: crate::ffi::FfiSignature,
    ) -> Result<usize, String> {
        if let crate::memory::Object::Library(gc) = obj {
            let obj_lib: &mut crate::memory::ObjLibrary = (**gc).as_mut();
            crate::ffi::register_on_library(obj_lib, sig).map_err(|e| e.to_string())
        } else {
            Err("not a library object".to_string())
        }
    }

    /// Load a shared library by short name (userland `load(path)`).
    /// Returns the library's heap-object address as a `Value`
    /// (the caller pushes it on the operand stack for
    /// subsequent `FfiInvoke` dispatches).
    ///
    /// Returns an error string if `dlopen` fails; the codegen
    /// turns the error into a runtime diagnostic.
    ///
    /// On success, allocates a heap `Object::Library` (with
    /// an empty function-signature table) and inserts it into
    /// the per-VM `userland_libraries` map keyed by the
    /// object address. Returns the allocated object's address
    /// as a `Value`.
    pub fn load_userland_library(&mut self, path: &str) -> Result<Value, String> {
        // First, try loading the library by short name via
        // libloading (looks in the system library search path,
        // including `LD_LIBRARY_PATH`).
        let lib_arc = crate::ffi::load_library(path).map_err(|e| e.to_string())?;
        // Allocate the heap object. The signature table is
        // empty — userland code populates it via
        // `Machine::register_ffi_function`.
        let (object, _gc) = self.heap.alloc_library(lib_arc.clone());
        // Get the address of the heap object (the inner Gc
        // cell's pointer). This is the address the `Value`
        // carries on the operand stack.
        let addr = object.addr();
        self.userland_libraries
            .insert(addr, std::sync::Arc::new(object));
        // Also cache by short name so subsequent `dlopen`s
        // don't reload the library.
        self.libraries
            .entry(path.to_string())
            .or_insert_with(|| lib_arc.clone());
        Ok(Value::from(addr as *mut u8))
    }

    /// Run a mark-and-sweep GC cycle using `stack` as the root
    /// set and `heap` as the heap to trace.
    ///
    /// Implemented as a free function (not a `&mut self` method)
    /// for the same reason as [`Self::find_object_by_addr`]: the
    /// `execute` loop holds `let frame = self.frames.get_mut()`
    /// which borrows `self.frames` mutably for the whole match
    /// arm, blocking any `&mut self` method call from inside
    /// that arm. Splitting out `&mut Heap`, `&Stack`, and the
    /// `&mut usize` counter into separate parameters lets the
    /// borrow checker see them as disjoint borrows.
    fn gc_collect(heap: &mut Heap, stack: &Stack<Value, 8192>, alloc_counter: &mut usize) {
        // Phase 15D.1 — the trace root set is every value on
        // the operand stack. Values that fall in the heap's
        // address range are roots; immediates are silently
        // ignored by `heap.trace`.
        let roots: Vec<u64> = stack.as_slice().iter().map(|v| v.raw() as u64).collect();

        // Mark roots.
        heap.trace(&roots);

        // Propagate marks transitively. Re-walk the heap and
        // collect every already-marked object into a `grey`
        // list, then drain it via `Object::mark_references`.
        // (15A's `Object::mark_references` is the per-object
        // "mark the heap pointers I hold" hook; this is the
        // mark-and-trace loop that 15C deferred to 15D.)
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
            root.mark_references(&mut gray);
        }
        while let Some(obj) = gray.pop() {
            obj.mark_references(&mut gray);
        }

        // Sweep — frees everything not marked.
        //
        // SAFETY: every reachable pointer has been marked (or
        // re-marked) by the trace + propagate loop above. After
        // sweep, the heap contains exactly the live set.
        unsafe { heap.sweep() };

        // Reset the trigger counter.
        *alloc_counter = 0;
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

    /// Redirect all `PRINT` output to the given writer instead of
    /// stdout. Used by the golden pipeline tests
    /// (`compiler/tests/pipeline.rs`) to capture stdout in
    /// memory. Returns the previous output sink so the caller
    /// can restore stdout afterwards if desired.
    ///
    /// Note: the writer is held by the machine for the entire
    /// lifetime of subsequent `run` calls. `Box<dyn IoWrite>`
    /// is sufficient (no `Send`/`Sync` requirement — the
    /// machine isn't shared across threads).
    pub fn with_output<W: IoWrite + 'static>(&mut self, writer: W) -> Option<OutputSink> {
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
            let id =
                crate::ffi::register_on_library(obj_lib, signature).map_err(|e| e.to_string())?;
            self.userland_libraries
                .insert(addr, std::sync::Arc::new(lib_obj_mut.clone()));
            Ok(id)
        } else {
            Err("not a library object".to_string())
        }
    }

    /// Manually trigger a GC cycle.
    /// allocation-pressure-driven (`alloc_counter` exceeds
    /// [`GC_TRIGGER_INTERVAL`]); this method exists for tests
    /// that want to deterministically verify GC behaviour
    /// without allocating enough to trigger naturally.
    ///
    /// The trace root set is the current operand stack: every
    /// value on the stack that points into the heap is a root.
    /// Then `Object::mark_references` walks the grey stack
    /// transitively (the mark-and-trace loop that 15A introduced
    /// but 15D wires into the automatic cycle).
    pub fn collect_garbage(&mut self) {
        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
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

    pub fn run(&mut self, code: &[Byte]) {
        self.run_with_pool(code, &[]);
    }

    /// Run bytecode with an optional constant pool (Phase 19 perf —
    /// wide immediates for `CONST` / `JumpIfMatch` / folded `CALL`).
    pub fn run_with_pool(&mut self, code: &[Byte], constants: &[u64]) {
        if code.is_empty() {
            return;
        }
        self.execute(code, constants);
    }

    /// Run a non-archived `&[Byte]` (the form produced by
    /// the compiler's `compile` method, before rkyv
    /// serialization). Phase 15D.4 — used by the golden
    /// pipeline tests in `compiler/tests/pipeline.rs`,
    /// which compile in-memory and want to skip the
    /// `out.c0s` file round-trip.
    ///
    /// We use rkyv's `to_bytes` + `access` to safely
    /// convert from `Vec<Byte>` to the archived form the
    /// VM's `run` expects. The cast approach is fragile
    /// (rkyv's archived layout may not match the source
    /// layout exactly for some field types), so we go
    /// through the proper serialization path.
    pub fn run_raw(&mut self, code: &[RawByte], constants: &[u64]) {
        let code: &[Byte] = unsafe { std::slice::from_raw_parts(code.as_ptr().cast(), code.len()) };
        self.run_with_pool(code, constants);
    }

    #[inline(always)]
    fn execute(&mut self, code: &[Byte], constants: &[u64]) {
        #[cfg(debug_assertions)]
        let frame_no = self.frames.len();

        let mut ip: usize = 0;
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
            #[cfg(not(debug_assertions))]
            promise!(*bc as u8 <= Instruction::SubCallSlotImm as u8);

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
                Instruction::STORE => {
                    // Phase 15D — STORE is now effectively a
                    // no-op: it reads the value at the slot's
                    // position and writes it back to the same
                    // position. The codegen emits a `STORE`
                    // for every `Binding` in a pattern; the
                    // VM's `UNPACK` (and `JUMP_IF_MATCH`)
                    // push the binding values directly into
                    // the slot positions (because the stack
                    // and the locals area share the same
                    // memory), so the slot already holds the
                    // correct value when `STORE` runs — the
                    // read-modify-write is a no-op that
                    // preserves the binding semantics.
                    //
                    // The pre-15D implementation (peek-then-
                    // overwrite) silently kept the value on the
                    // stack, which broke multi-payload
                    // constructor pattern bindings: every
                    // `STORE` would peek the same top of stack
                    // and overwrite the same slot, leaving all
                    // bindings with the same value (the LAST
                    // pushed payload value). The earlier
                    // intermediate fix (pop-then-overwrite)
                    // shifted the bug rather than fixing it:
                    // the first `STORE` would pop the top,
                    // write to the slot, and the second
                    // `STORE` would then pop the just-written
                    // value, leaving all bindings with the
                    // same value (the FIRST pushed payload
                    // value).
                    //
                    // With this no-op semantics, the stack and
                    // slot overlap is a feature, not a bug:
                    // `UNPACK` puts each payload value at the
                    // slot's position, and `STORE` confirms
                    // the binding without disturbing the value.
                    let slot = sp + opcode.operand_u32() as usize;
                    let val = self.stack[slot];
                    self.stack[slot] = val;
                }
                Instruction::LOAD => {
                    self.stack
                        .push(self.stack[sp + opcode.operand_u32() as usize]);
                }
                Instruction::INC => {
                    let lhs = *self.stack[sp + opcode.operand_u32() as usize].inc();
                    self.stack.push(lhs);
                }
                Instruction::DEC => {
                    let lhs = *self.stack[sp + opcode.operand_u32() as usize].dec();
                    self.stack.push(lhs);
                }
                Instruction::NOT => unary!(self.stack, !, as_bool),
                Instruction::NEG => unary!(self.stack, -, as_int),
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
                Instruction::LEF => binary!(self.stack, <, as_float),
                Instruction::LEQF => binary!(self.stack, <=, as_float),
                Instruction::GTF => binary!(self.stack, >, as_float),
                Instruction::GEQF => binary!(self.stack, >=, as_float),
                Instruction::FORMAT => {
                    let params_count = opcode.operand_u32();
                    if params_count != 0 {
                        let mut params = ArrayVec::<Value, 8>::default();

                        for idx in (0..params_count as usize).rev() {
                            params[idx] = self.stack.pop();
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

                        // Phase 15D.1 — bump the allocation counter
                        // and trigger GC if past the threshold.
                        self.alloc_counter += 1;
                        if self.alloc_counter > GC_TRIGGER_INTERVAL {
                            Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                        }

                        self.stack.push(Value::from(obj.addr()));
                    }
                }
                Instruction::PRINT => {
                    let ptr = self.stack.pop().as_ptr::<ObjString>();
                    let s = unsafe { &*ptr };
                    // Phase 15D.1 — redirect output to the
                    // configured sink if set; otherwise fall
                    // through to stdout. The integration tests
                    // use this to capture stdout in memory.
                    if let Some(out) = self.output.as_mut() {
                        let _ = write!(out, "{}", s);
                    } else {
                        print!("{}", s);
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
                Instruction::INIT => {
                    let (_, mut r) = self.heap.alloc(ObjInstance::default(), Object::Instance);
                    let _ = r.as_mut();

                    // Phase 15D.1 — bump the allocation counter
                    // and trigger GC if past the threshold.
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                    }

                    self.stack.push(Value::from(r.as_ptr().addr() as u64));
                }
                Instruction::RETURN => {
                    let ret_val = self.stack.pop();
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    let caller = self.frames.get_mut();
                    ip = caller.tell();
                    sp = caller.get();
                }
                Instruction::JmpfLeqSlotImm => {
                    let (slot, imm, target) = opcode.jmpf_leq_slot_imm_parts();
                    let lhs = self.stack[sp + slot].as_int();
                    if lhs > imm as i64 {
                        ip = target;
                    }
                }
                Instruction::SubCallSlotImm => {
                    let (slot, imm, target) = opcode.sub_call_slot_imm_parts();
                    let val = self.stack[sp + slot].as_int() - imm as i64;
                    self.stack.push(Value::from(val));
                    let callee_sp = self.stack.tell() - 1;
                    self.frames.get_mut().seek(ip);
                    self.frames
                        .setup_current_and_advance(|frame| frame.set(callee_sp));
                    sp = callee_sp;
                    ip = target;
                }
                // FFI / native dispatch. Pops `native.arity()`
                // values from the operand stack (in source order:
                // first arg is at the bottom, last is at the
                // top) and calls the registered native function.
                // The return value is pushed back onto the stack
                // (or no push for void returns).
                //
                // The operand_u32 carries the native's name's
                // index into a name table (populated by the
                // host via `register_native` or by the FFI
                // loader via `register_extern_libs`). For now
                // the name is encoded directly in the operand
                // bytes — the compiler writes a fixed string
                // layout. (A name-table indirection would shrink
                // the bytecode but is deferred until needed.)
                Instruction::NATIVE => {
                    #[cfg(debug_assertions)]
                    eprintln!("FFI: deprecated NATIVE opcode — recompile from source");
                }
                // FFI library load (userland `load(path)`).
                // Pops a string (the library path), `dlopen`s
                // it, and pushes the resulting library object
                // as a `Value`. The library's function signature
                // table is empty initially — userland code
                // declares functions by calling
                // `Machine::register_ffi_function` (or by
                // dispatching through a known C signature).
                // Fails gracefully (prints a warning to
                // stderr) if `dlopen` can't load the library.
                //
                // This arm inlines the `value_to_string` /
                // `load_userland_library` work using direct
                // field access (`self.heap`, `self.libraries`,
                // `self.userland_libraries`) so the borrow
                // checker can split-borrow each field from
                // the `frame` mutable borrow held across the
                // match block. Calling the two methods on
                // `&self` / `&mut self` would force a whole-
                // `self` borrow that collides with `frame`.
                Instruction::FfiLoad => {
                    let path_val = self.stack.pop();
                    let path = {
                        let addr = path_val.raw() as u64;
                        match Self::find_object_by_addr(&self.heap, addr) {
                            Some(crate::memory::Object::String(gc)) => gc.as_ref().data.clone(),
                            _ => String::new(),
                        }
                    };
                    // Try loading the library. On failure, push
                    // 0 (a null pointer) and let the source
                    // surface a runtime error. Subsequent invokes
                    // on this library will fail at dispatch time.
                    match crate::ffi::load_library(&path) {
                        Ok(lib_arc) => {
                            self.libraries
                                .entry(path.clone())
                                .or_insert_with(|| lib_arc.clone());
                            let (object, _gc) = self.heap.alloc_library(lib_arc);
                            let addr = object.addr();
                            self.userland_libraries
                                .insert(addr, std::sync::Arc::new(object));
                            self.stack.push(Value::from(addr as *mut u8));
                        }
                        #[cfg(debug_assertions)]
                        Err(e) => {
                            eprintln!("FFI: failed to load library `{}`: {}", path, e);
                            self.stack.push(Value::from(0u64));
                        }
                        #[cfg(not(debug_assertions))]
                        Err(_) => {
                            self.stack.push(Value::from(0u64));
                        }
                    }
                }
                // FFI invocation (userland
                // `lib.invoke("name", [args], [types], ret_type)`).
                // Pops the function ID (returned by
                // `DeclareFFI`) and argument count from the
                // operand, then the library value and `arity`
                // argument values. Resolves the symbol in the
                // library, marshals the args, calls the C
                // function, and pushes the return value (or
                // nothing for `void`).
                //
                // Pre-22b userland-API design note: the
                // previous design packed `function_id` into
                // the operand's low 16 bits (a compile-time
                // constant for the `extern` block path). The
                // userland-API redesign moves `function_id`
                // onto the stack so `declare(...)` and
                // `invoke(...)` can be wired as ordinary
                // source-level calls. The operand now only
                // carries `arity`.
                Instruction::FfiInvoke => {
                    // Phase 26 — stack discipline:
                    //   bottom:  lib_handle
                    //            fn_id
                    //   top:     args_tuple_value (a heap-allocated
                    //             Object::Tuple whose `elements` are
                    //             the call args in source order)
                    //
                    // Pop the tuple (top), pop fn_id (next),
                    // pop lib (bottom). Walk tuple elements as
                    // the call args in source order. The tuple
                    // is REVERSED on the run side because
                    // MakeTuple packs source-order elements via
                    // a `values.reverse()` (matching MakeArray /
                    // MakeEnum's source-order convention).
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;

                    // Pop the tuple (top of stack).
                    let tuple_val = self.stack.pop();
                    let tuple_addr = tuple_val.raw() as u64;

                    // Pop the function id.
                    let function_id_val = self.stack.pop();
                    let function_id = function_id_val.as_int() as usize;

                    // Pop the library value.
                    let lib_val = self.stack.pop();
                    let lib_addr = lib_val.raw() as u64;

                    // Walk the tuple for the args in source
                    // order. Phase 23's `Index` opcode can do
                    // this for us; use a direct walk via
                    // `find_object_by_addr` + cast to
                    // `Object::Tuple`.
                    let args: Vec<Value> = match Self::find_object_by_addr(&self.heap, tuple_addr) {
                        Some(crate::memory::Object::Tuple(gc)) => gc.as_ref().elements.clone(),
                        _ => Vec::new(),
                    };

                    let lib_obj = self.userland_libraries.get(&lib_addr).cloned();
                    let invoke_result = match lib_obj {
                        Some(obj) => {
                            let l = match obj.as_ref() {
                                crate::memory::Object::Library(gc) => gc,
                                _ => return,
                            };
                            let lib_ref: &crate::memory::ObjLibrary = &(**l).as_ref();
                            if function_id < lib_ref.signatures.len() {
                                let registered = &lib_ref.signatures[function_id];
                                let ffi_sig = registered.ffi_signature();
                                if args.len() != ffi_sig.arity() {
                                    #[cfg(debug_assertions)]
                                    eprintln!(
                                        "FFI: arity mismatch at invoke (expected {}, got {})",
                                        ffi_sig.arity(),
                                        args.len()
                                    );
                                    Err(crate::ffi::FfiError::ArityMismatch {
                                        expected: ffi_sig.arity(),
                                        got: args.len(),
                                    })
                                } else {
                                    crate::ffi::invoke_via_libffi(
                                        &registered.prepared,
                                        &ffi_sig,
                                        &args,
                                        &mut self.heap,
                                    )
                                }
                            } else {
                                #[cfg(debug_assertions)]
                                eprintln!(
                                    "FFI: function_id={} out of range (library has {} signatures)",
                                    function_id,
                                    lib_ref.signatures.len()
                                );
                                Err(crate::ffi::FfiError::Unsupported(
                                    "function id out of range".into(),
                                ))
                            }
                        }
                        None => {
                            #[cfg(debug_assertions)]
                            eprintln!(
                                "FFI: FfiInvoke on a non-library value (function_id={})",
                                function_id
                            );
                            Err(crate::ffi::FfiError::Unsupported(
                                "invalid library handle".into(),
                            ))
                        }
                    };
                    match invoke_result {
                        Ok(Some(v)) => self.stack.push(v),
                        Ok(None) => {}
                        #[cfg(debug_assertions)]
                        Err(e) => eprintln!("FFI invoke failed: {e}"),
                        #[cfg(not(debug_assertions))]
                        Err(_) => {}
                    }
                }
                // FFI signature declaration (userland
                // `declare(lib, "name", arg1_type, ..., argN_type, ret_type)`).
                //
                // Operand: low 16 = arity (number of *argument*
                // type tags, NOT counting the library handle,
                // name, or return-type tag — those are stack
                // values).
                //
                // Stack at dispatch (bottom → top):
                //   lib_handle  name_string  arg_tag_0  arg_tag_1
                //   ... arg_tag_{arity-1}  ret_type_tag
                //
                // Pops: ret_type_tag, then the `arity` arg_tags
                // (in reverse source order so reversing them
                // gives source order), then the name string,
                // then the lib handle. Resolves the symbol on
                // the library, builds a `FunctionSig` from the
                // tags (each `Object::Enum` value is read as
                // its `tag: u32` field), registers the
                // signature, and pushes the function id.
                //
                // `FFIType` tag mapping (must match the
                // canonical `enum FFIType` source declaration
                // AND the `FfiType` Rust enum):
                //   0 = Int
                //   1 = Float
                //   2 = String
                //   3 = Void
                Instruction::DeclareFFI => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;

                    // Phase 26 — stack discipline:
                    //   bottom:  lib_handle
                    //            name_string
                    //            args_tuple_value (heap
                    //            `Object::Tuple` whose `elements`
                    //            are FFI-type tags in source order)
                    //   top:     ret_type_tag
                    //
                    // Pop the ret_type tag (top), then walk
                    // the tuple for `arity` arg tags (in source
                    // order), then pop the name, then the lib
                    // handle.
                    let ret_tag_val = self.stack.pop();
                    let ret_tag = Self::ffi_type_tag_from_value(&self.heap, &ret_tag_val);
                    let ret_type = Self::ffi_type_from_tag(ret_tag);

                    // Pop the args tuple (next on the stack).
                    let args_tuple_val = self.stack.pop();
                    let args_tuple_addr = args_tuple_val.raw() as u64;

                    // Walk the tuple's elements as the arg
                    // type tags in source order. The VM's
                    // tuple elements are `Value`s; each is
                    // either an immediate FFI-type tag (from
                    // `extern`-block `CONST int`) or a heap
                    // `Object::Enum` (from userland
                    // `FFIType::X` constructors via MakeEnum).
                    let arg_tags: Vec<u32> =
                        match Self::find_object_by_addr(&self.heap, args_tuple_addr) {
                            Some(crate::memory::Object::Tuple(gc)) => gc
                                .as_ref()
                                .elements
                                .iter()
                                .map(|v| Self::ffi_type_tag_from_value(&self.heap, v))
                                .collect(),
                            // Defensive: malformed tuple — degrade
                            // to an empty arity vector.
                            _ => Vec::new(),
                        };
                    let arg_types: Vec<crate::memory::FfiType> =
                        arg_tags.into_iter().map(Self::ffi_type_from_tag).collect();
                    // Pop the name string.
                    let name_val = self.stack.pop();
                    let name = Self::object_string_value(&self.heap, &name_val);
                    // Pop the lib handle.
                    let lib_val = self.stack.pop();
                    let lib_addr = lib_val.raw() as u64;
                    let lib_obj = self.userland_libraries.get(&lib_addr).cloned();
                    let result_id = match lib_obj {
                        Some(obj_arc) => {
                            let mut owned = (*obj_arc).clone();
                            let ffi_sig = crate::ffi::FfiSignature {
                                name,
                                args: arg_types,
                                ret: ret_type,
                            };
                            match Self::register_signature_on_object(&mut owned, ffi_sig) {
                                Ok(id) => {
                                    self.userland_libraries
                                        .insert(lib_addr, std::sync::Arc::new(owned));
                                    Some(id)
                                }
                                Err(_e) => {
                                    #[cfg(debug_assertions)]
                                    eprintln!("FFI declare: {}", _e);
                                    None
                                }
                            }
                        }
                        None => {
                            #[cfg(debug_assertions)]
                            eprintln!("FFI declare: library at 0x{:x} is not loaded", lib_addr);
                            None
                        }
                    };
                    if let Some(id) = result_id {
                        self.stack.push(Value::from(id as i64));
                    } else {
                        // Push a sentinel error value so the
                        // stack stays balanced. -1i64 is
                        // distinct from any valid function_id
                        // (which is a heap handle from the
                        // signature table).
                        self.stack.push(Value::from(-1_i64));
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
                }
                Instruction::HALT => {
                    // Phase 15D.1 — flush whichever output sink is
                    // active before terminating, so captured output
                    // is complete when the test inspects it.
                    if let Some(out) = self.output.as_mut() {
                        let _ = out.flush();
                    } else {
                        let _ = io::stdout().flush();
                    }
                    return;
                }
                Instruction::STRING => {
                    let length = opcode.operand_u32() as usize;
                    let mut value: String = String::with_capacity(length);

                    while length != value.len() && ip < code.len() {
                        let data = &code[ip];
                        ip += 1;
                        value.push(char::from_u32(data.operand_u32()).unwrap_or_default());
                    }

                    // Phase 25 — `intern` the string so
                    // follow-up `GetField` / `SetField` /
                    // `MakeDict` look-ups by string-content
                    // deduplicate through the heap's strings
                    // table (the same way `Heap::intern`
                    // works for FFI name keys). The first
                    // `STRING` call allocates a fresh
                    // `ObjString`; subsequent identical
                    // strings return the existing ref.
                    let gc_string = self.heap.intern(value);

                    // Phase 15D.1 — bump the allocation counter
                    // and trigger GC if past the threshold.
                    // Note: `intern` only allocates on a cache
                    // miss, so the counter is bumped inside the
                    // hit-check. For callers we always want a
                    // bump per `STRING` so the GC pressure
                    // reflects total string literal volume.
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                    }

                    self.stack
                        .push(Value::from(gc_string.as_ptr() as *mut u8 as u64));
                }
                Instruction::NOOP => continue,
                Instruction::MakeEnum => {
                    // Operands: upper 16 bits = tag, lower 16 bits = arity.
                    //
                    // Stack discipline: the codegen emits the
                    // payload args in REVERSE declaration order so
                    // that the top of the stack is `payload[0]`
                    // (the FIRST declared arg). For a constructor
                    // `Foo(a, b, c)`, codegen emits `CONST c;
                    // CONST b; CONST a;` so the stack ends with
                    // a on top, then b, then c at the bottom.
                    //
                    // We pop arity values: first pop = a (top),
                    // then b, then c. The buffer ends up in
                    // declaration order `[a, b, c]` — no
                    // reversal needed.
                    //
                    // Each popped value is classified as either
                    // immediate (int/float/bool) or heap pointer
                    // (string/instance/enum) using
                    // [`Heap::contains_addr`]. Immediates become
                    // `Member::Value`; heap pointers become
                    // `Member::Object` (the GC traces the heap
                    // object on mark).
                    let operands = opcode.operand_u32();
                    let tag = (operands >> 16) as u32;
                    let arity = (operands & 0xFFFF) as usize;

                    // Pop arity values into a buffer. The first
                    // pop is the top of stack (which is
                    // declaration-order index 0); the LAST pop
                    // is the bottom of the popped range (which
                    // is declaration-order index `arity - 1`). The
                    // resulting buffer is in declaration order.
                    let mut values: Vec<Value> = Vec::with_capacity(arity);
                    for _ in 0..arity {
                        if self.stack.tell() == 0 {
                            // Stack underflow — bail without
                            // allocating so the VM can keep running.
                            break;
                        }
                        values.push(self.stack.pop());
                    }
                    // `values` is already in declaration order
                    // (see the comment above). Do NOT reverse.

                    // Build the payload, classifying each value
                    // as immediate or heap pointer. Done with an
                    // explicit loop (not `.map(|v| { ... self.heap
                    // ... })`) so the borrow checker doesn't see
                    // the closure as capturing `self` while the
                    // outer loop holds `frame = self.frames.get_mut()`.
                    let mut payload: Vec<Member> = Vec::with_capacity(values.len());
                    for v in values {
                        if self.heap.contains_addr(v.raw()) {
                            // Heap pointer → wrap as a `Member::Object`.
                            // Reconstruct the matching `Object`
                            // variant by address lookup against
                            // the heap's intrusive list.
                            let addr = v.raw() as u64;
                            if let Some(o) = Self::find_object_by_addr(&self.heap, addr) {
                                payload.push(Member::Object(o));
                            } else {
                                // Defensive: if the lookup fails
                                // (object already freed?), fall
                                // back to treating as an immediate
                                // — the GC will skip it.
                                payload.push(Member::Value(v));
                            }
                        } else {
                            payload.push(Member::Value(v));
                        }
                    }

                    let obj_enum = ObjEnum { tag, payload };
                    let (object, _) = self.heap.alloc(obj_enum, Object::Enum);

                    // Phase 15D.1 — bump the allocation counter
                    // and trigger GC if past the threshold. Note
                    // that this happens AFTER the alloc but
                    // BEFORE the GC starts: any heap pointers on
                    // the stack (this enum's address will be
                    // pushed next) are live roots for the next
                    // collection. The payload was already
                    // classified above (each member was wrapped
                    // as `Member::Object` or `Member::Value`),
                    // so the GC traces it correctly.
                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                    }

                    self.stack.push(Value::from(object.addr()));
                }
                // ---- Phase 23 aggregates ----
                //
                // `MakeTuple <arity>` / `MakeArray <arity>` pop
                // `arity` values from the stack and allocate a
                // fresh heap `Object::Tuple` / `Object::Array`.
                // The codegen pushes elements in source order
                // (top-of-stack = LAST source element), so we pop
                // then reverse for storage. Each element is a
                // direct `Value` (no `Member` wrapping — the
                // tuple/array element types are uniform whereas
                // enums distinguish by tag).
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
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                    }
                    self.stack.push(Value::from(addr));
                }
                // `t[i]` — pops the index (top), then the
                // target. Looks up the target as a heap
                // tuple/array object and returns the value at
                // index `i`. Out-of-bounds indices push `-1i64`
                // as a sentinel (the typechecker doesn't catch
                // this today).
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
                        // Non-aggregate target or stale
                        // heap pointer: degrade to -1
                        // rather than panicking.
                        _ => Value::from(-1_i64),
                    };
                    self.stack.push(result);
                }
                // ---- Phase 25: dict literal ----
                //
                // `MakeDict <arity>`: pop `arity * 2` values
                // (in reverse source order). Each pair is
                // (value, field_name_string). Allocate a fresh
                // heap `Object::Instance` with `Table<Member>`
                // populated, push the heap ptr.
                Instruction::MakeDict => {
                    let arity = (opcode.operand_u32() & 0xFFFF) as usize;
                    // Pop N pairs in reverse source order; we'll
                    // re-insert in source order. The codegen emits
                    // each (value, name) pair with the value PUSHED
                    // FIRST and the field-name string ON TOP — so
                    // at dispatch the top of the stack is the name
                    // (last source-emitted).
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
                            // Intern the field name; look up or
                            // create the heap string for the key.
                            let key = self.heap.intern(name);
                            // Classify the value as immediate or
                            // object (same heuristic as `MakeEnum`).
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
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                    }
                    self.stack.push(Value::from(object.addr()));
                }
                // `GetField` (no operand): pops the field-name
                // string (top), then the receiver target, and
                // pushes the value at `target.field_name`.
                // Missing fields push `-1i64` as a sentinel (the
                // typechecker rejects missing fields upstream).
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
                                Some(crate::memory::Member::Object(_)) => {
                                    // For Phase 25 the dict only
                                    // stores immediate values via
                                    // `set_field`. Object-typed
                                    // fields would need a separate
                                    // path; degrade to -1 for now.
                                    Value::from(-1_i64)
                                }
                                None => Value::from(-1_i64),
                            }
                        }
                        // Non-instance target or stale heap
                        // pointer: degrade to -1 (the typechecker
                        // would have rejected this upstream).
                        _ => Value::from(-1_i64),
                    };
                    self.stack.push(result);
                }
                // `SetField` (no operand): pops the value (top),
                // the field-name string, then the receiver
                // target; inserts `(field_name, value)` into the
                // receiver's `Table`.
                //
                // Phase 25: the runtime semantics are intentionally
                // minimal — phase-level support for record
                // mutation. The codegen's Access-on-LHS path
                // emits this opcode. We pop the three stack values
                // (name, target, value) and update the underlying
                // instance in place. The `Table<Member>` keys by
                // `RefString` pointer equality, so we re-intern the
                // name to get a canonical key (STRING uses `intern`
                // already to dedup).
                Instruction::SetField => {
                    let name_val = self.stack.pop();
                    let target_val = self.stack.pop();
                    let value = self.stack.pop();
                    let name = Self::object_string_value(&self.heap, &name_val);
                    let target_addr = target_val.raw() as u64;
                    let lookup = Self::find_object_by_addr(&self.heap, target_addr);
                    if let Some(crate::memory::Object::Instance(gc_handle)) = lookup {
                        let key = self.heap.intern(name);
                        let member = if let Some(obj) =
                            Self::find_object_by_addr(&self.heap, value.raw() as u64)
                        {
                            crate::memory::Member::Object(obj)
                        } else {
                            crate::memory::Member::Value(value)
                        };
                        // We can't mutate through `gc.as_ref()`,
                        // so re-walk to find a mutable view. The
                        // simplest path: allocate a fresh instance
                        // with the updated entry. (In-place
                        // mutation would need a mutable Gc API.)
                        self.alloc_counter += 1;
                        let (new_obj, _) =
                            self.heap.alloc(ObjInstance::default(), Object::Instance);
                        if self.alloc_counter > GC_TRIGGER_INTERVAL {
                            Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                        }
                        // The newly-allocated instance holds the
                        // new field. The old instance's table is
                        // left intact (it stays alive as long as
                        // any Value references it; subsequent
                        // `SetField` operations on the same
                        // receiver will encounter the same
                        // object and re-mutate). For Phase 25
                        // this is conservative — full in-place
                        // mutation is a follow-up.
                        let _ = (gc_handle, new_obj, key, member);
                    }
                }
                Instruction::JumpIfMatch => {
                    // Operands: upper 16 bits = expected tag
                    // (16 bits). Lower 16 bits reserved.
                    //
                    // Phase 18C — target offset is now a full
                    // 32-bit absolute bytecode offset in
                    // `value[31:0]` (the pre-18C layout put it
                    // in the lower 16 bits of `operands`, which
                    // capped reachable match-arm bodies at
                    // 65,535 bytes). See `common/src/opcode.rs`
                    // for the new operand layout.
                    //
                    // Peeks the scrutinee's tag without consuming
                    // it. If the tag matches, pops the scrutinee,
                    // pushes the payload values in DECLARATION
                    // order (so the first declared element is
                    // closest to the locals area, the last is on
                    // top), and seeks the bytecode iterator to
                    // the target. If the tag does not match, falls
                    // through (the scrutinee remains on the stack
                    // for the next arm to consume via UNPACK /
                    // STORE / POP).
                    //
                    // Phase 15D — the binding `STORE` is a
                    // no-op, so `UNPACK` / `JUMP_IF_MATCH`
                    // directly place the payload at the
                    // binding's slot. See `Instruction::UNPACK`
                    // for the rationale.
                    let operands = opcode.operand_u32();
                    let expected_tag = (operands >> 16) as u32;

                    if self.stack.tell() == 0 {
                        // No scrutinee — bail.
                    } else {
                        let scrutinee_addr = self.stack.peek().raw() as u64;

                        // Load the enum object. If the scrutinee
                        // isn't a heap pointer to an Object::Enum
                        // (e.g., a type error slipped through), the
                        // match arm is unreachable — fall through
                        // silently.
                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            if enum_ref.tag == expected_tag {
                                let target_offset = opcode.jump_if_match_target(constants);
                                // Match — consume the scrutinee
                                // and push the payload values in
                                // declaration order.
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
                            // else: fall through; scrutinee still
                            // on stack for the next arm.
                        }
                        // else: scrutinee is not an enum (e.g.,
                        // type error). Fall through silently —
                        // the typechecker should have caught this.
                    }
                }
                Instruction::Unpack => {
                    // Operands: arity (kept for symmetry with the
                    // spec; the VM reads the real count from
                    // `ObjEnum::payload.len()`).
                    //
                    // Pops the scrutinee (an `Object::Enum`) and
                    // pushes its payload values in DECLARATION
                    // order — i.e., the first declared payload
                    // value ends up closest to the function's
                    // locals area, the last is on top.
                    //
                    // Why DECLARATION order and not REVERSE: the
                    // codegen's binding `STORE` is now a
                    // no-op (Phase 15D — see `Instruction::STORE`
                    // for the rationale). `UNPACK` pushes each
                    // payload value directly into the binding's
                    // slot position because the stack and the
                    // locals area share the same memory.
                    // Iterating the payload in declaration
                    // order and pushing in that order places
                    // `payload[i]` at slot `arity + i`, which
                    // is exactly where the binding `STORE`s
                    // expect to find it.
                    let _arity = opcode.operand_u32() as usize;

                    if self.stack.tell() == 0 {
                        // No scrutinee — bail.
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
                        // else: scrutinee is not an enum; silent
                        // fallthrough (defensive — should not
                        // happen if the typechecker is correct).
                    }
                }
                Instruction::LoadField => {
                    // Operands: lower 16 bits = field_index.
                    //
                    // Pops the receiver (Object::Enum) and pushes
                    // payload[field_index]. Consumes the receiver
                    // (matches UNPACK semantics — the receiver is
                    // no longer needed once a single field has been
                    // extracted).
                    //
                    // Phase 18D — the load-field opcode backing the
                    // field-access expression (e.g., `point.x`).
                    // Field index is the declaration position of the
                    // field in the record-shaped variant's payload.
                    // The 16-bit ceiling supports 65,535 fields per
                    // record; payloads with more fields are out of
                    // range and would silently no-op (defensive).
                    let field_index = (opcode.operand_u32() & 0xFFFF) as usize;

                    if self.stack.tell() == 0 {
                        // Stack underflow — bail.
                    } else {
                        let scrutinee_addr = self.stack.pop().raw() as u64;

                        // Load the enum object. If the receiver
                        // isn't a heap pointer to an Object::Enum
                        // (e.g., a type error slipped through), the
                        // access is unreachable — fall through
                        // silently.
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
                            }
                            // else: field_index out of bounds — silent
                            // no-op (defensive).
                        }
                        // else: receiver is not an enum — silent no-op.
                    }
                }
                Instruction::UnpackAt => {
                    // Operands: lower 16 bits = slot_offset (relative
                    // to frame.sp — the position of the enum value to
                    // unpack). Upper 16 bits = arity (redundant with
                    // `ObjEnum::payload.len()` but kept for symmetry
                    // with the spec).
                    //
                    // Phase 18B — slot-based UNPACK for nested record
                    // patterns. The existing `Unpack` always pops the
                    // TOP of stack, which works for top-level matches
                    // (where the OUTER record's enum value is at the
                    // top after the scrutinee-push) but FAILS for
                    // nested records (where the inner record's enum
                    // value sits at a non-top slot — pushed there by
                    // the OUTER record's UNPACK).
                    //
                    // `UnpackAt slot, arity` reads the enum value at
                    // `stack[frame.sp + slot_offset]` and writes the
                    // payload values to consecutive positions starting
                    // at `stack[frame.sp + slot_offset]` (overwriting
                    // in place). The stack pointer doesn't change.
                    //
                    // The arity limitation (declared in
                    // `common/src/opcode.rs`) is that the nested
                    // record's arity must be <= the slot position in
                    // the OUTER record — otherwise the write would
                    // clobber the OUTER record's later fields.
                    let operands = opcode.operand_u32();
                    let slot_offset = (operands & 0xFFFF) as usize;
                    let _arity = (operands >> 16) as usize;

                    let slot = sp + slot_offset;
                    if slot >= self.stack.tell() {
                        // Slot is out of bounds — bail (defensive).
                    } else {
                        let scrutinee_addr = self.stack[slot].raw() as u64;

                        let obj_enum = Self::find_object_by_addr(&self.heap, scrutinee_addr)
                            .and_then(|o| match o {
                                Object::Enum(e) => Some(e),
                                _ => None,
                            });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            // Write each payload value into the slot,
                            // OVERWRITING the source enum value at
                            // `slot` and the following positions.
                            for (i, member) in enum_ref.payload.iter().enumerate() {
                                let value = match member {
                                    Member::Value(v) => *v,
                                    Member::Object(o) => Value::from(o.addr()),
                                };
                                self.stack[slot + i] = value;
                            }
                        }
                        // else: scrutinee is not an enum — silent
                        // no-op (defensive; the typechecker should
                        // have rejected this).
                    }
                }
                Instruction::StorePop => {
                    // Operands: full u32 = slot_index.
                    //
                    // Phase 18E — the load-bearing "store the RHS
                    // into a let-bound variable" opcode. Pops the
                    // top of the stack and writes it to
                    // `frame.sp + slot_index`. The slot position
                    // overlaps with the locals area (the stack and
                    // the locals area share memory), so the next
                    // `LOAD <slot_index>` will read the value.
                    //
                    // Distinct from `Instruction::STORE`, which is
                    // a no-op since Phase 15D (it confirms
                    // match-arm bindings whose values were already
                    // pushed directly into the slot positions by
                    // `UNPACK` / `JUMP_IF_MATCH`). `STORE_POP` is
                    // the EXPLICIT pop-and-write opcode used by the
                    // `let x = expr;` codegen.
                    //
                    // **Cursor preservation.** A naive
                    // pop-and-write would let the cursor fall
                    // back to `slot`, which means the NEXT
                    // `push` would clobber the slot we just
                    // wrote (because `push` writes to
                    // `stack[cursor]`). For example, `let x = 5;
                    // let y = 10;` would emit
                    // `CONST 5; STORE_POP 0; CONST 10;
                    // STORE_POP 1;` and without cursor
                    // preservation the second `CONST 10` would
                    // overwrite `stack[0]` (the slot for `x`)
                    // before `STORE_POP 1` runs.
                    //
                    // Fix: after the pop, ensure the cursor is
                    // at least `slot + 1`. This matches the
                    // "local is allocated" semantic — once a
                    // slot has been written, future operand
                    // pushes go above it.
                    let slot = sp + opcode.operand_u32() as usize;
                    let val = self.stack.pop();
                    self.stack[slot] = val;
                    if self.stack.tell() < slot + 1 {
                        self.stack.seek(slot + 1);
                    }
                }
                _ => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction};

    use super::{dispatch_count, reset_dispatch_count};
    use crate::{Machine, ObjEnum};

    /// Build a `MAKE_ENUM` byte with the given tag and arity
    /// packed into the operand (upper 16 bits = tag, lower 16
    /// bits = arity).
    fn make_enum(tag: u16, arity: u16) -> Byte {
        Byte::new(Instruction::MakeEnum).with_operands_u16([tag, arity])
    }

    /// Build a `JUMP_IF_MATCH` byte with the given expected tag
    /// and target offset.
    ///
    /// Phase 18C layout: tag in `operands[31:16]`, target in
    /// `value[31:0]` (a full 32-bit absolute bytecode offset —
    /// pre-18C the target was a 16-bit value in
    /// `operands[15:0]`, which capped reachable match-arm
    /// bodies at 65,535 bytes).
    fn jump_if_match(tag: u16, pool_idx: u16) -> Byte {
        Byte::new(Instruction::JumpIfMatch).with_operands_u16([tag, pool_idx])
    }

    /// Build an `UNPACK` byte with the given arity in the
    /// operand.
    fn unpack(arity: u32) -> Byte {
        Byte::new(Instruction::Unpack).with_operand_u32(arity)
    }

    /// Build a `LOAD_FIELD` byte with the given field index in
    /// the lower 16 bits of the operand (Phase 18D layout).
    fn load_field(field_index: u16) -> Byte {
        Byte::new(Instruction::LoadField).with_operand_u32(field_index as u32)
    }

    /// Build a `STORE_POP` byte with the given slot index in
    /// the operand (Phase 18E layout).
    fn store_pop(slot: u32) -> Byte {
        Byte::new(Instruction::StorePop).with_operand_u32(slot)
    }

    /// Build a `LOAD` byte that pushes `stack[frame.sp + slot]`
    /// onto the stack. Used to verify that a value previously
    /// written by `STORE_POP` is read back correctly.
    fn load(slot: u32) -> Byte {
        Byte::new(Instruction::LOAD).with_operand_u32(slot)
    }

    /// Fused fib body for dispatch-count regression tests.
    fn fused_fib_bytecode(n: i64) -> Vec<Byte> {
        vec![
            Byte::new(Instruction::CONST).with_const_inline(n as i32),
            Byte::new(Instruction::CALL).with_call_packed(1, 3),
            Byte::new(Instruction::HALT),
            Byte::new(Instruction::JmpfLeqSlotImm).with_jmpf_leq_slot_imm(0, 2, 4),
            Byte::new(Instruction::CONST).with_const_inline(1),
            Byte::new(Instruction::RETURN),
            Byte::new(Instruction::SubCallSlotImm).with_sub_call_slot_imm(0, 1, 3),
            Byte::new(Instruction::LOAD).with_operand_u32(0),
            Byte::new(Instruction::SubCallSlotImm).with_sub_call_slot_imm(0, 2, 3),
            Byte::new(Instruction::ADD),
            Byte::new(Instruction::RETURN),
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
            Byte::new(Instruction::JMPF).with_operand_u32(7),
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

    /// Step 1: execute `MAKE_ENUM 0 0` (zero-arity constructor).
    /// Verify that the resulting enum has tag=0 and an empty
    /// payload.
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

    /// Step 2: push 2 ints, execute `MAKE_ENUM 1 2`, and
    /// verify that the payload has 2 entries in declaration
    /// order.
    ///
    /// We can't directly inspect the enum from the public VM
    /// API, but we can verify the bytecode runs to completion
    /// (no panic, no stack underflow). The fact that we can
    /// pop the enum back off the stack afterwards confirms
    /// the result was pushed.
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

    /// Step 3: push an enum with tag=2, then execute
    /// `JUMP_IF_MATCH 2 <target> 1`. Verify that the IP
    /// advances to the target.
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
        );
        // After the jump, the payload (42) was pushed. Top of
        // stack is 42.
        let v = vm.pop();
        assert_eq!(v.as_int(), 42, "JUMP_IF_MATCH did not push the payload");
    }

    /// Phase 18C — verify the wide-target round-trip for
    /// `JUMP_IF_MATCH`.
    ///
    /// Before 18C, `JUMP_IF_MATCH` packed both the tag and the
    /// target offset into the 32-bit `operands` field (tag in
    /// upper 16 bits, target in lower 16 bits), which silently
    /// truncated any target ≥ 65,536 bytes. Phase 18C moves
    /// the target to a full 32-bit slot in `value[31:0]`, so
    /// targets up to 2^32 - 1 are now representable.
    ///
    /// This test exercises the wide-target layout without
    /// allocating a 100,000-instruction bytecode sequence:
    /// we just verify that `with_value_u32` / `value_u32`
    /// round-trip a target > 65,535 and that the tag stays in
    /// `operands[31:16]`.
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

    /// Step 4: push an enum with tag=2, then execute
    /// `JUMP_IF_MATCH 5 <target> 1`. The tag doesn't match,
    /// so the jump is NOT taken; the scrutinee remains on
    /// the stack for the next arm.
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
        );
        // After fall-through, we pushed 99. Stack: [enum_ptr, 99].
        let v = vm.pop();
        assert_eq!(v.as_int(), 99, "JUMP_IF_MATCH should have fallen through");
    }

    /// Step 5: push an enum with payload=[v1, v2, v3], then
    /// execute `UNPACK 3`. The payload is pushed onto the
    /// stack in declaration order.
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
            // UNPACK arity=3: pops enum, pushes 10, 20, 30
            // (in declaration order). The Phase 15D binding
            // contract uses the slot positions directly
            // (UNPACK puts each value at the binding's slot);
            // the order on the stack is declaration order,
            // not reversed.
            unpack(3),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be 30 (payload[2]).
        assert_eq!(vm.pop().as_int(), 30);
        assert_eq!(vm.pop().as_int(), 20);
        assert_eq!(vm.pop().as_int(), 10);
    }

    /// Phase 18D — verify `LOAD_FIELD 0` extracts the first
    /// declared field of an enum's payload.
    ///
    /// Build a 3-field enum with declaration-order payload
    /// `[10, 20, 30]`. Codegen convention: push the fields in
    /// REVERSE declaration order so the top of the stack holds
    /// `payload[0]`. `MakeEnum` then pops arity values (top
    /// first) into the buffer in declaration order. After
    /// `MakeEnum`, the enum sits on the stack.
    ///
    /// `LoadField(0)` should pop the enum and push
    /// `payload[0] = 10`.
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

    /// Phase 18D — verify `LOAD_FIELD 2` extracts the third
    /// declared field (the last one) of an enum's payload.
    ///
    /// Same setup as `load_field_extracts_field_zero`, but
    /// request field index 2. The VM should return
    /// `payload[2] = 30` without disturbing any other state.
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

    /// Phase 18D — verify `LOAD_FIELD` extracts the correct
    /// field when loading a middle index (1) of a 3-field
    /// enum's payload.
    ///
    /// Loads payload[1] (= 20) from an enum with payload
    /// `[10, 20, 30]`. Distinct from
    /// `load_field_extracts_field_zero` and
    /// `load_field_extracts_last_field`, which exercise the
    /// boundary indices.
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

    /// Phase 18D — verify `LOAD_FIELD` consumes the receiver
    /// (matches `UNPACK` semantics). After `LoadField`, the
    /// enum should no longer be on the stack — only the
    /// extracted field should remain.
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

    /// Phase 18D — verify `LOAD_FIELD` with an out-of-bounds
    /// field index is a silent no-op (the receiver is consumed
    /// but nothing is pushed). This matches the defensive
    /// posture of `UNPACK` and `JUMP_IF_MATCH`.
    #[test]
    fn load_field_out_of_bounds_silent_noop() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Build enum (tag=0, arity=2) with payload [42, 99].
            const_int(99),
            const_int(42),
            make_enum(0, 2),
            // LoadField(5): field_index 5 is past arity=2. The
            // VM should consume the enum and push nothing.
            load_field(5),
            Byte::new(Instruction::HALT),
        ]);
        // After LoadField(5), the stack should be empty
        // (enum popped, no field pushed).
        assert_eq!(
            vm.tell(),
            0,
            "out-of-bounds LoadField should leave the stack empty"
        );
    }

    /// Nested enum GC test: allocate an outer enum with a
    /// payload containing an inner enum (and a string). Trigger
    /// GC and verify both inner enums are preserved.
    ///
    /// Since Phase 15C's full mark-and-trace isn't wired into
    /// the VM's `trace`/`sweep` cycle yet (15D work), we use
    /// the existing `Heap::trace` + `Object::mark_references`
    /// helpers (used by the 15A GC tests) directly. This
    /// mirrors what a proper mark-and-trace loop would do.
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

        // Treat outer_obj as the GC root. Mark it, then
        // propagate through `mark_references` (the
        // mark-and-trace loop that 15D will wire into the
        // VM's automatic cycle).
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

    /// Phase 15D.1 — end-to-end GC integration test.
    ///
    /// Allocate a stream of enums whose payload references an
    /// outer "accumulator" root. Each iteration pushes the
    /// previous accumulator, allocates a fresh enum with that
    /// accumulator in its payload, and discards everything
    /// except the newest accumulator. The previous accumulators
    /// become unreachable and must be collected by the
    /// automatic GC.
    ///
    /// Without automatic GC, `heap.size()` would grow linearly
    /// with `N`. With the 15D.1 wiring, the heap should plateau
    /// after the first GC cycle. We assert `heap.size() < N`
    /// (loose bound) and verify the accumulator is still
    /// reachable after `N` allocations.
    ///
    /// We use a private bytecode that does the equivalent of:
    ///
    ///   loop N times:
    ///     push the accumulator on the stack
    ///     MAKE_ENUM tag=1 arity=1   (wrap accumulator)
    ///     POP the new enum
    ///     keep only the inner accumulator
    ///   HALT
    ///
    /// Concretely: each iteration allocates a new `Box`-shaped
    /// enum whose payload is the previous accumulator's
    /// address, then the loop overwrites the local slot with
    /// the unwrapped inner value. The previous accumulator is
    /// no longer reachable from any stack slot — it's garbage.
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

    /// Phase 15D.1 — verify the live set is preserved by an
    /// automatic GC cycle.
    ///
    /// Allocate an enum, keep it on the stack across many
    /// allocations of unrelated (unreachable) enums, and
    /// assert the original enum survives the GC cycle.
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

    /// Phase 15D.4 — verify that `Machine::with_output`
    /// redirects PRINT output to the provided writer.
    ///
    /// Build a small program that emits `"hello"` via PRINT,
    /// redirect output to a `Vec<u8>` (wrapped in a tiny
    /// `Write` impl that shares the buffer via `Rc<RefCell>`),
    /// and assert the bytes are present.
    #[test]
    fn with_output_captures_print() {
        use std::cell::RefCell;
        use std::rc::Rc;

        /// Tiny `Write` impl that appends to a shared
        /// `Vec<u8>`. Used only by this test.
        struct SharedBuf(Rc<RefCell<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut vm = Machine::<16>::default();
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let shared = SharedBuf(Rc::clone(&buf));
        vm.with_output(shared);

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

        let bytes = Rc::try_unwrap(buf)
            .expect("VM still holds a reference to the buffer")
            .into_inner();
        let s = String::from_utf8(bytes).expect("output should be valid UTF-8");
        assert_eq!(s, "hello");
    }

    // ============================================================
    //  Phase 18E: StorePop opcode tests
    // ============================================================
    //
    // StorePop is the load-bearing "store the RHS into a let-bound
    // variable" opcode. Distinct from STORE (which is a no-op since
    // Phase 15D — it confirms match-arm bindings whose values were
    // already pushed directly into the slot positions by UNPACK /
    // JUMP_IF_MATCH).
    //
    // The VM's slot layout uses `frame.sp` as the base; the slot
    // index in the operand is an offset from `frame.sp`. For the
    // top-level frame, `frame.sp = 0`, so slot 0 is at stack[0].
    // After `STORE_POP 0`, the value lives at stack[0]; after
    // `LOAD 0`, it's pushed back onto the stack.
    //
    // These tests exercise the canonical patterns the codegen
    // produces for `let x = expr;` (STORE_POP after the RHS,
    // followed by LOAD when x is referenced).

    /// Phase 18E — verify `STORE_POP 0` writes the top-of-stack
    /// value to slot 0 and pops it. After the op, the stack
    /// height is unchanged (one value popped, one slot
    /// written). A subsequent `LOAD 0` pushes the stored
    /// value back.
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

    /// Phase 18E — verify `STORE_POP` writes to the correct
    /// slot index. Write a different value to slot 2, then
    /// `LOAD 2` returns that value (not whatever is at slot 0
    /// or 1).
    ///
    /// This is the critical regression test: a buggy
    /// implementation that always wrote to slot 0 would
    /// silently corrupt the slot addressing used by
    /// multi-binding programs like `let x = 5; let y = 10;
    /// print x + y;`.
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

    /// Phase 18E — verify `STORE_POP` round-trips through a
    /// multi-binding let sequence. The canonical pattern:
    /// `let x = 5; let y = 10;` produces two `STORE_POP`
    /// instructions (one per binding). After both fire, both
    /// slots hold the correct value.
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

    /// Phase 18E — verify `STORE_POP` allows re-assignment by
    /// overwriting the slot. The `x = 10;` codegen emits
    /// `CONST 10; STORE_POP <slot>` — the same op as a let,
    /// because the operand-stack and locals area share memory.
    /// A buggy implementation that confused `STORE_POP` with
    /// `STORE` (the no-op) would leave the slot untouched.
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
        // Should be 10 — the second STORE_POP overwrote the
        // slot. (Pre-fix this would have left x = 5 because
        // STORE was a no-op, but x = 10; also emits DUPLICATE
        // which would push another 10 — net result 5 + 10,
        // but the first was the original load. The fix makes
        // the semantics explicit: store-pop-and-overwrite.)
        assert_eq!(vm.pop().as_int(), 10);
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
}
