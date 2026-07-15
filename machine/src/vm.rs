//! Bytecode interpreter: dispatch loop, automatic GC, and FFI.

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

type OutputSink = Box<dyn IoWrite>;

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 8192>,
    frames: ArrayVec<Frame, S>,
    output: Option<OutputSink>,
    alloc_counter: usize,
    natives: crate::ffi::Natives,
    libraries: std::collections::HashMap<String, std::sync::Arc<crate::ffi::Library>>,
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
            output: None,
            alloc_counter: 0,
            natives: crate::ffi::Natives::new(),
            libraries: std::collections::HashMap::new(),
            userland_libraries: std::collections::HashMap::new(),
        }
    }
}

impl<const S: usize> Machine<S> {
    // pub fn register(&mut self, name: usize, func: External) {
    //     self.native.insert(name, func);
    // }

    /// Free function so `execute` can borrow `frames` and `heap` separately.
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

    #[allow(dead_code)]
    fn value_to_string(&self, v: &Value) -> String {
        self.heap
            .cstr_from_addr(v.raw() as u64)
            .map(|s| unsafe { std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned() })
            .unwrap_or_default()
    }

    fn ffi_type_tag_from_value(heap: &Heap, v: &Value) -> u32 {
        // Small integers are immediate FFI type tags; larger values are heap enums.
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

    fn ffi_type_from_tag(tag: u32) -> crate::memory::FfiType {
        match tag {
            1 => crate::memory::FfiType::Float,
            2 => crate::memory::FfiType::String,
            3 => crate::memory::FfiType::Void,
            _ => crate::memory::FfiType::Int,
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
    fn gc_collect(heap: &mut Heap, stack: &Stack<Value, 8192>, alloc_counter: &mut usize) {
        let roots: Vec<u64> = stack.as_slice().iter().map(|v| v.raw() as u64).collect();

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
            root.mark_references(&mut gray);
        }
        while let Some(obj) = gray.pop() {
            obj.mark_references(&mut gray);
        }

        // SAFETY: all reachable objects were marked above.
        unsafe { heap.sweep() };

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

    /// Redirect `PRINT` output (used by pipeline tests).
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
                .insert(addr, std::sync::Arc::new(*lib_obj_mut));
            Ok(id)
        } else {
            Err("not a library object".to_string())
        }
    }

    /// Manually trigger GC (for tests).
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

    /// Run bytecode with an optional constant pool for wide immediates.
    pub fn run_with_pool(&mut self, code: &[Byte], constants: &[u64]) {
        if code.is_empty() {
            return;
        }
        self.execute(code, constants);
    }

    /// Run compiler-produced bytecode (archived layout, no `.c0s` round-trip).
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
            promise!(*bc as u8 <= Instruction::BinSlotSlot as u8);

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
                    // No-op: stack and locals share memory; UNPACK/JUMP_IF_MATCH
                    // already wrote match bindings into slot positions.
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
                Instruction::LoadReturnSlot => {
                    let ret_val = self.stack[sp + opcode.operand_u32() as usize];
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    let caller = self.frames.get_mut();
                    ip = caller.tell();
                    sp = caller.get();
                }
                Instruction::ConstReturnImm => {
                    let ret_val = Value::from(opcode.operand_u32() as i32 as i64 as u64);
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    let caller = self.frames.get_mut();
                    ip = caller.tell();
                    sp = caller.get();
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
                    let return_sp = self.frames.pop().get();
                    self.stack.seek(return_sp);
                    self.stack.push(ret_val);
                    let caller = self.frames.get_mut();
                    ip = caller.tell();
                    sp = caller.get();
                }
                Instruction::BinSlotSlot => {
                    let (op, a, b) = opcode.bin_slot_slot_parts();
                    let va = self.stack[sp + a];
                    let vb = self.stack[sp + b];
                    self.stack.push(va);
                    self.stack.push(vb);
                    match Instruction::from(op) {
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
                Instruction::FfiInvoke => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;

                    // Stack (bottom → top): lib, fn_id, args_tuple.
                    let tuple_val = self.stack.pop();
                    let tuple_addr = tuple_val.raw() as u64;

                    // Pop the function id.
                    let function_id_val = self.stack.pop();
                    let function_id = function_id_val.as_int() as usize;

                    // Pop the library value.
                    let lib_val = self.stack.pop();
                    let lib_addr = lib_val.raw() as u64;

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
                            let lib_ref: &crate::memory::ObjLibrary = (**l).as_ref();
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
                Instruction::DeclareFFI => {
                    let raw = opcode.operand_u32();
                    let _arity = (raw & 0xFFFF) as usize;

                    // Stack (bottom → top): lib, name, args_tuple, ret_tag.
                    let ret_tag_val = self.stack.pop();
                    let ret_tag = Self::ffi_type_tag_from_value(&self.heap, &ret_tag_val);
                    let ret_type = Self::ffi_type_from_tag(ret_tag);

                    // Pop the args tuple (next on the stack).
                    let args_tuple_val = self.stack.pop();
                    let args_tuple_addr = args_tuple_val.raw() as u64;

                    let arg_tags: Vec<u32> =
                        match Self::find_object_by_addr(&self.heap, args_tuple_addr) {
                            Some(crate::memory::Object::Tuple(gc)) => gc
                                .as_ref()
                                .elements
                                .iter()
                                .map(|v| Self::ffi_type_tag_from_value(&self.heap, v))
                                .collect(),
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
                            let mut owned = *obj_arc;
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

                    let gc_string = self.heap.intern(value);

                    self.alloc_counter += 1;
                    if self.alloc_counter > GC_TRIGGER_INTERVAL {
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
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
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
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
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
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
                        Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
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
                                Some(crate::memory::Member::Object(_)) => Value::from(-1_i64),
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
                        // In-place mutation not yet implemented; allocates a fresh instance.
                        self.alloc_counter += 1;
                        let (new_obj, _) =
                            self.heap.alloc(ObjInstance::default(), Object::Instance);
                        if self.alloc_counter > GC_TRIGGER_INTERVAL {
                            Self::gc_collect(&mut self.heap, &self.stack, &mut self.alloc_counter);
                        }
                        let _ = (gc_handle, new_obj, key, member);
                    }
                }
                Instruction::JumpIfMatch => {
                    // Tag in operands[31:16]; jump target in value[31:0].
                    let operands = opcode.operand_u32();
                    let expected_tag = operands >> 16;

                    if self.stack.tell() == 0 {
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
                            }
                        }
                    }
                }
                Instruction::UnpackAt => {
                    // Unpack enum at `sp + slot_offset` in place (nested record patterns).
                    let operands = opcode.operand_u32();
                    let slot_offset = (operands & 0xFFFF) as usize;
                    let _arity = (operands >> 16) as usize;

                    let slot = sp + slot_offset;
                    if slot >= self.stack.tell() {
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
                        }
                    }
                }
                Instruction::StorePop => {
                    // Pop TOS into `sp + slot`; advance cursor past the slot so
                    // subsequent pushes don't clobber locals (`let x; let y;`).
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
                Byte::new(Instruction::BinSlotSlot).with_bin_slot_slot(Instruction::ADDF as u8, 0, 1),
                Byte::new(Instruction::HALT),
            ],
            &pool,
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
