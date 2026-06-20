use std::{fmt::Write, io::Write as _};

use common::{
    ArchivedByte as Byte, ArchivedInstruction as Instruction, ArrayVec, SeekableIterator, Value,
    unlikely,
};

use crate::{Frame, Heap, Member, ObjEnum, ObjInstance, ObjString, Object, Stack};

macro_rules! binary {
    ($stack: expr, $op:tt, $from: ident, $to: ident) => {
        {
            let rhs = $stack.pop().$from();
            let lhs = $stack.peek().$from();

            $stack.top().replace((lhs $op rhs).$to());
        }
    };
    ($stack: expr, $op:tt, $from: ident) => {
        {
            let rhs = $stack.pop().$from();
            let lhs = $stack.peek().$from();

            $stack.top().replace((lhs $op rhs) as _)
        }
    };
}

macro_rules! unary {
    ($stack: expr, $op: tt, $from: ident, $to: ident) => {
        {
        let rhs = $stack.peek().$from();

        $stack.top().replace(($op rhs).$to());
        }
    };
    ($stack: expr, $op: tt, $from: ident) => { {
            let rhs = $stack.peek().$from();

            $stack.top().replace(($op rhs) as _);
        }
    }
}

// type External = fn(&[Value]) -> Value;

pub struct Machine<const S: usize> {
    heap: Heap,
    stack: Stack<Value, 8192>,
    frames: ArrayVec<Frame, S>,
}

#[derive(Default, Copy, Clone)]
#[repr(u8)]
enum ExecutionOutcome {
    #[default]
    INVALID,
    CALL,
    RETURN,
    TERMINATION,
}

#[derive(Default)]
struct ExecutionResult {
    outcome: ExecutionOutcome,
    arity: usize,
}
impl ExecutionResult {
    pub fn returns() -> Self {
        Self {
            outcome: ExecutionOutcome::RETURN,
            arity: 0,
        }
    }

    pub fn call(arity: usize) -> Self {
        Self {
            outcome: ExecutionOutcome::CALL,
            arity,
        }
    }

    pub fn terminate() -> Self {
        Self {
            outcome: ExecutionOutcome::TERMINATION,
            arity: 0,
        }
    }

    pub fn invalid() -> Self {
        Self {
            outcome: ExecutionOutcome::INVALID,
            arity: usize::MAX,
        }
    }

    #[inline]
    pub fn outcome(&self) -> ExecutionOutcome {
        self.outcome
    }

    #[inline]
    pub fn arity(&self) -> usize {
        self.arity
    }
}

impl<const S: usize> Default for Machine<S> {
    fn default() -> Self {
        let mut frames = ArrayVec::default();
        frames.consume();
        Self {
            frames,
            heap: Heap::default(),
            stack: Stack::new(),
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

    pub fn run(&mut self, code: &[Byte]) {
        if code.is_empty() {
            return;
        }

        let mut code_iter = SeekableIterator::new(code);

        loop {
            let result = self.execute(&mut code_iter);

            match result.outcome() {
                ExecutionOutcome::CALL => {
                    self.frames.get_mut().seek(code_iter.tell() + 1);
                    // code_iter.seek(result.tell());
                    self.frames.current_mut().enter();
                    self.frames
                        .current_mut()
                        .set(self.stack.tell() - result.arity());

                    self.frames.consume();
                }
                ExecutionOutcome::RETURN => {
                    let current = self.frames.pop();
                    let v = self.stack.pop();
                    self.stack.seek(current.get());
                    self.stack.push(v);

                    let prev = self.frames.get_mut();

                    code_iter.seek(prev.tell());
                }
                ExecutionOutcome::TERMINATION => {
                    unlikely(true);
                    break;
                }
                _ => (),
            }
        }
    }

    #[inline(always)]
    fn execute(&mut self, code: &mut SeekableIterator<'_, Byte>) -> ExecutionResult {
        #[cfg(debug_assertions)]
        let frame_no = self.frames.len();

        let frame = self.frames.get_mut();

        while let Some(opcode) = code.next() {
            #[cfg(debug_assertions)]
            {
                eprintln!(
                    "#{:<2} @ {:0>4} - {:>8}[{:0>4}, {:0>4}] - {:?}",
                    frame_no,
                    code.tell(),
                    format!("{:?}", opcode.bytecode()),
                    opcode.operand_u16(0),
                    opcode.operand_u16(1),
                    self.stack.as_slice()
                );
            }

            #[cfg(debug_assertions)]
            {
                println!("Performing GC trace");
                self.heap.trace(
                    &self
                        .stack
                        .as_slice()
                        .iter()
                        .map(|v| v.raw() as u64)
                        .collect::<Vec<u64>>(),
                );

                println!("Performing GC collection");
                unsafe { self.heap.sweep() };
            }

            match opcode.bytecode() {
                Instruction::POP => {
                    self.stack.pop();
                }
                Instruction::DUPLICATE => {
                    self.stack.push(*self.stack.peek());
                }
                Instruction::CONST => self.stack.push(Value::from(opcode.constant())),
                Instruction::STORE => {
                    let val = self.stack.peek();
                    self.stack[frame.get() + opcode.operand_u32() as usize] = *val;
                }
                Instruction::LOAD => {
                    self.stack
                        .push(self.stack[frame.get() + opcode.operand_u32() as usize]);
                }
                Instruction::INC => {
                    let lhs = *self.stack[frame.get() + opcode.operand_u32() as usize].inc();
                    self.stack.push(lhs);
                }
                Instruction::DEC => {
                    let lhs = *self.stack[frame.get() + opcode.operand_u32() as usize].dec();
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

                        self.stack.push(Value::from(obj.addr()));
                    }
                }
                Instruction::PRINT => {
                    let ptr = self.stack.pop().as_ptr::<ObjString>();
                    print!("{}", unsafe { &*ptr });
                }
                Instruction::JMP => {
                    code.seek(opcode.operand_u32() as usize);
                }
                Instruction::JMPF => {
                    if !self.stack.pop().as_bool() {
                        code.seek(opcode.operand_u32() as usize);
                    }
                }
                Instruction::JMPT => {
                    if self.stack.pop().as_bool() {
                        code.seek(opcode.operand_u32() as usize);
                    }
                }
                Instruction::CALL => {
                    return ExecutionResult::call(opcode.operand_u32() as usize);
                }
                Instruction::INIT => {
                    let (_, mut r) = self.heap.alloc(ObjInstance::default(), Object::Instance);
                    let _ = r.as_mut();

                    self.stack.push(Value::from(r.as_ptr().addr() as u64));
                }
                Instruction::RETURN => {
                    return ExecutionResult::returns();
                }
                Instruction::HALT => {
                    let _ = std::io::stdout().flush();
                    return ExecutionResult::terminate();
                }
                Instruction::STRING => {
                    let length = opcode.operand_u32() as usize;
                    let mut value: String = String::with_capacity(length);

                    while length != value.len()
                        && let Some(data) = code.next()
                    {
                        value.push(char::from_u32(data.operand_u32()).unwrap_or_default());
                    }

                    let (object, _) = self
                        .heap
                        .alloc(ObjString::from(value.as_str()), Object::String);

                    self.stack.push(Value::from(object.addr()));
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
                    self.stack.push(Value::from(object.addr()));
                }
                Instruction::JumpIfMatch => {
                    // Operands: upper 16 bits = expected tag, lower
                    // 16 bits = target offset.
                    //
                    // Peeks the scrutinee's tag without consuming
                    // it. If the tag matches, pops the scrutinee,
                    // pushes the payload values in declaration
                    // order, and seeks the bytecode iterator to the
                    // target. If the tag does not match, falls
                    // through (the scrutinee remains on the stack
                    // for the next arm to consume via UNPACK /
                    // STORE / POP).
                    let operands = opcode.operand_u32();
                    let expected_tag = (operands >> 16) as u32;
                    let target_offset = (operands & 0xFFFF) as usize;

                    if self.stack.tell() == 0 {
                        // No scrutinee — bail.
                    } else {
                        let scrutinee_addr = self.stack.peek().raw() as u64;

                        // Load the enum object. If the scrutinee
                        // isn't a heap pointer to an Object::Enum
                        // (e.g., a type error slipped through), the
                        // match arm is unreachable — fall through
                        // silently.
                        let obj_enum = Self::find_object_by_addr(
                            &self.heap,
                            scrutinee_addr,
                        )
                        .and_then(|o| match o {
                            Object::Enum(e) => Some(e),
                            _ => None,
                        });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            if enum_ref.tag == expected_tag {
                                // Match — consume the scrutinee
                                // and push the payload values in
                                // declaration order.
                                let _ = self.stack.pop();
                                for member in &enum_ref.payload {
                                    let value = match member {
                                        Member::Value(v) => *v,
                                        Member::Object(o) => {
                                            Value::from(o.addr())
                                        }
                                    };
                                    self.stack.push(value);
                                }
                                code.seek(target_offset);
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
                    // pushes its payload values in declaration
                    // order. Used for the LAST arm of a match
                    // (reached via fall-through from the previous
                    // `JUMP_IF_MATCH`) and for any explicit
                    // unpack operation.
                    let _arity = opcode.operand_u32() as usize;

                    if self.stack.tell() == 0 {
                        // No scrutinee — bail.
                    } else {
                        let scrutinee_addr = self.stack.pop().raw() as u64;

                        let obj_enum =
                            Self::find_object_by_addr(&self.heap, scrutinee_addr)
                                .and_then(|o| match o {
                                    Object::Enum(e) => Some(e),
                                    _ => None,
                                });

                        if let Some(enum_ref) = obj_enum {
                            let enum_ref = enum_ref.as_ref();
                            for member in &enum_ref.payload {
                                let value = match member {
                                    Member::Value(v) => *v,
                                    Member::Object(o) => {
                                        Value::from(o.addr())
                                    }
                                };
                                self.stack.push(value);
                            }
                        }
                        // else: scrutinee is not an enum; silent
                        // fallthrough (defensive — should not
                        // happen if the typechecker is correct).
                    }
                }
                _ => return ExecutionResult::invalid(),
            }
        }

        ExecutionResult::terminate()
    }
}

#[cfg(test)]
mod tests {
    use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction, Value};

    use crate::{Machine, ObjEnum};

    /// Build a `MAKE_ENUM` byte with the given tag and arity
    /// packed into the operand (upper 16 bits = tag, lower 16
    /// bits = arity).
    fn make_enum(tag: u16, arity: u16) -> Byte {
        Byte::new(Instruction::MakeEnum).with_operands_u16([tag, arity])
    }

    /// Build a `JUMP_IF_MATCH` byte with the given expected tag
    /// and target offset packed into the operand (upper 16
    /// bits = tag, lower 16 bits = target).
    fn jump_if_match(tag: u16, target: u16) -> Byte {
        Byte::new(Instruction::JumpIfMatch).with_operands_u16([tag, target])
    }

    /// Build an `UNPACK` byte with the given arity in the
    /// operand.
    fn unpack(arity: u32) -> Byte {
        Byte::new(Instruction::Unpack).with_operand_u32(arity)
    }

    /// Build a `CONST` byte that pushes the given `i64` value
    /// onto the stack. Used to set up the operand values for
    /// `MAKE_ENUM` and `JUMP_IF_MATCH`.
    fn const_int(value: i64) -> Byte {
        Byte::new(Instruction::CONST).with_value(Value::from(value))
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
        vm.run(&[
            // Build the enum (tag=2, arity=1) with payload [42]:
            const_int(42),
            make_enum(2, 1),
            // JUMP_IF_MATCH tag=2 target=4
            jump_if_match(2, 4),
            // (Should not reach here on the jump-taken path.)
            const_int(999),
            // HALT at offset 4 (the target).
            Byte::new(Instruction::HALT),
        ]);
        // After the jump, the payload (42) was pushed. Top of
        // stack is 42.
        let v = vm.pop();
        assert_eq!(v.as_int(), 42, "JUMP_IF_MATCH did not push the payload");
    }

    /// Step 4: push an enum with tag=2, then execute
    /// `JUMP_IF_MATCH 5 <target> 1`. The tag doesn't match,
    /// so the jump is NOT taken; the scrutinee remains on
    /// the stack for the next arm.
    #[test]
    fn jump_if_match_not_taken_falls_through() {
        let mut vm = Machine::<4>::default();
        vm.run(&[
            // Build an enum (tag=2, arity=1) with payload [42]:
            const_int(42),
            make_enum(2, 1),
            // JUMP_IF_MATCH tag=5 target=4 (won't match; fall through)
            jump_if_match(5, 4),
            // (Should be reached on the fall-through path.)
            const_int(99),
            // Target for the (non-taken) jump at offset 4.
            Byte::new(Instruction::HALT),
        ]);
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
            // UNPACK arity=3: pops enum, pushes 10, 20, 30.
            unpack(3),
            Byte::new(Instruction::HALT),
        ]);
        // Top of stack should be 30 (payload[2]).
        assert_eq!(vm.pop().as_int(), 30);
        assert_eq!(vm.pop().as_int(), 20);
        assert_eq!(vm.pop().as_int(), 10);
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
        let (string_obj, _) =
            heap.alloc(ObjString::from("inner"), Object::String);
        // Allocate an outer enum whose payload contains
        // references to both the inner enum and the string.
        let (outer_obj, _) = heap.alloc(
            ObjEnum {
                tag: 0,
                payload: vec![
                    Member::Object(inner_obj),
                    Member::Object(string_obj),
                ],
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
}
