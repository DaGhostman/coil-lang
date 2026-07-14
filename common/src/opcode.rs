use std::fmt::Debug;

use rkyv::{Archive, Deserialize, Serialize};

use crate::Value;

#[repr(u8)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug), derive(Clone), derive(Copy))]
pub enum Instruction {
    // -- Special
    #[default]
    HALT,
    NOOP,
    DUPLICATE,
    POP,
    CONST,
    STORE,
    LOAD,
    CALL,
    RETURN,
    JMP,
    JMPT,
    JMPF,
    STRING,
    DATA,
    INC,
    DEC,

    // Arithmetic
    ADD,
    SUB,
    MUL,
    DIV,
    MOD,
    ADDF,
    SUBF,
    MULF,
    DIVF,
    MODF,
    NOT,
    NEG,
    AND,
    OR,
    SHL,
    SHR,
    XOR,
    EQ,
    NEQ,
    LE,
    LEQ,
    LEF,
    LEQF,
    GT,
    GEQ,
    GTF,
    GEQF,
    // -- Keyword
    PRINT,
    FORMAT,
    STRINGIFY,
    NATIVE,
    INIT,
    SET,

    // ---- Phase 15C: sum types and pattern matching ----
    //
    // CRITICAL: these are APPENDED (not inserted) to keep the
    // `#[repr(u8)]` discriminant values of every prior opcode
    // stable. Inserting a new variant before `SET` would shift
    // the numeric value of `SET` (and every later opcode) and
    // silently corrupt every `.0s` archive ever compiled.
    //
    // Operand layout:
    // - `MAKE_ENUM`:    upper 16 bits = tag, lower 16 bits = arity.
    // - `JUMP_IF_MATCH`: upper 16 bits = expected tag (16 bits),
    //   lower 16 bits reserved. The target offset lives in
    //   `value[31:0]` (a full 32-bit absolute bytecode offset —
    //   see Phase 18C layout below). The payload arity is read
    //   from the runtime enum object (`ObjEnum::payload.len()`)
    //   so no separate arity field is needed.
    // - `UNPACK`:       full u32 = arity (redundant with
    //   `ObjEnum::payload.len()` but kept for symmetry with the
    //   spec; the VM reads it from the enum at runtime).
    //
    // JumpIfMatch layout (Phase 18C: 32-bit target):
    //   operands[31:16] = expected tag (16 bits)
    //   operands[15:0]  = reserved (write 0)
    //   value[31:0]     = absolute bytecode target offset (32 bits)
    //   value[63:32]    = reserved (write 0)
    //
    // LoadField layout (Phase 18D):
    //   operands[15:0]  = field_index (declaration position in the record payload)
    //   operands[31:16] = reserved (write 0)
    //
    // Pops the receiver (an Object::Enum on the stack) and pushes
    // payload[field_index]. Consumes the receiver (matches UNPACK semantics).
    //
    // ---- Phase 18E: let-bound variables ----
    //
    // StorePop layout (Phase 18E):
    //   operands[31:0] = slot_index (absolute offset into the operand-stack/locals area)
    //
    // Pops the top of the stack and writes it to `frame.sp + slot_index`.
    // This is the load-bearing "store the RHS into the let-bound variable"
    // opcode. Distinct from Instruction::STORE (which is a no-op since
    // Phase 15D — it confirms match-arm bindings whose values were already
    // pushed directly into the slot positions by UNPACK / JUMP_IF_MATCH).
    MakeEnum,
    JumpIfMatch,
    Unpack,
    LoadField,
    StorePop,

    // ---- Phase 18B: nested record patterns ----
    //
    // UNPACK_AT (slot-based UNPACK for nested record patterns):
    //   operands[15:0]  = slot offset (relative to frame.sp — the
    //                     position of the enum value to unpack)
    //   operands[31:16] = arity (redundant with
    //                     `ObjEnum::payload.len()` but kept for
    //                     symmetry with the spec; the VM reads the
    //                     real count from the enum at runtime)
    //
    // Reads `stack[frame.sp + slot_offset]` as an enum value and
    // writes the payload values to consecutive positions starting
    // at `stack[frame.sp + slot_offset]` (overwriting in place).
    //
    // Distinct from `Unpack` which always pops the TOP of the
    // stack — `UnpackAt` reads from an arbitrary slot, so nested
    // record patterns (where the inner record's enum value sits
    // at a non-top slot after the OUTER record's UNPACK pushed
    // its fields) can be bound to the right slot positions.
    //
    // Limitation (Phase 18B spec): the arity of the nested record
    // must be <= the field's position in the OUTER record's
    // decl_order. A 2-field nested record at position 1 would
    // clobber the OUTER record's position-2 field. Programs with
    // multi-field nested records interleaved with non-nested
    // OUTER fields would need a scratch-area scheme (deferred to
    // 19+).
    UnpackAt,

    // ---- Phase 22b: userland FFI (APPENDED — see append-only contract below) ----
    //
    // CRITICAL: these are appended AFTER every existing variant.
    // Inserting them earlier would shift every later variant's
    // `#[repr(u8)]` discriminant and silently corrupt every `.c0s`
    // archive ever compiled.
    //
    // `FfiLoad` pops a string (the library path), calls
    // `dlopen`, allocates a heap `Object::Library` wrapping
    // the loaded `Library`, and pushes the library's address
    // as a `Value`. Signatures are registered later via
    // `DeclareFFI` at runtime.
    FfiLoad,
    //
    // `FfiInvoke` (Phase 26 tuple form) — stack at dispatch
    // (bottom → top): lib_handle, fn_id, args_tuple.
    // Resolves the function by id in the library's signature
    // table and calls it via libffi using the explicit signature
    // prepared at declare time. No signature guessing.
    FfiInvoke,

    // ---- Phase 22b append: DeclareFFI ----
    //
    // Operand layout (must be APPENDED — see the comment
    // block at the top of the enum):
    //   low 16  = arity (number of *argument* type tags, not
    //             counting the library handle, name, or
    //             return-type tag — those are always present
    //             as additional stack values).
    //   high 16 = reserved (0)
    //
    // Stack at dispatch (Phase 26 — bottom → top):
    //   lib_handle  name_string  args_tuple  ret_type_tag
    //
    // Pops ret tag, walks args_tuple for arg type tags, pops
    // name and lib handle. Resolves the symbol via dlsym,
    // prepares a libffi CIF, and registers the signature.
    // Pushes the function id (or -1 on failure).
    DeclareFFI,

    // ---- Phase 23: aggregates (tuples + arrays + indexing) ----
    //
    // `MakeTuple <arity>` and `MakeArray <arity>`: pop
    // `arity` values from the stack (in source order —
    // top-of-stack is the LAST source element; the VM
    // reverses for source order) and allocate a fresh heap
    // `Object::Tuple` (or `Object::Array`). The address is
    // pushed as a `Value` (a heap pointer).
    //
    // Operands:
    //   low 16  = arity (number of source-order elements)
    //   high 16 = reserved (0)
    //
    // `Index` (no operand): pops the index (top), then the
    // target, and pushes the element at `target[index]`.
    // Out-of-bounds indices push `Value::from(-1i64)` as a
    // sentinel (the typechecker doesn't catch this today).
    MakeTuple,
    MakeArray,
    Index,

    // ---- Phase 25: dict / anonymous record ----
    //
    // `MakeDict <arity>`: pop `arity * 2` values from the
    // stack in reverse source order (codegen emits pairs
    // in source order — the value is pushed first, then the
    // field-name string is pushed on top). Each pair is
    // `(value, field_name_string)`. Allocates a fresh heap
    // `Object::Instance` (Phase 25 reuses the existing
    // class-instance representation for dict storage; the
    // `Table<Member>` keyed by interned field-name string).
    //
    // Operands:
    //   low 16  = arity (number of fields)
    //   high 16 = reserved (0)
    MakeDict,

    // `GetField` (no operand): pops the field-name string
    // (top), then the receiver target, and pushes the
    // value at `target.field_name` (looked up via the
    // runtime's `Heap::cstr_from_addr` for the string +
    // `Heap` walk to find the `Object::Instance`). Missing
    // fields push `Value::from(-1i64)` as a sentinel (the
    // typechecker should reject missing fields upstream).
    GetField,

    // `SetField` (no operand): pops the value (top), the
    // field-name string, and the receiver target; inserts
    // `(field_name, value)` into the receiver's `Table`.
    // Distinct from `STORE` which is a no-op reserved for
    // match-arm bindings (Phase 15D). Phase 25's record
    // mutation path.
    SetField,

    // ---- Phase 30: host native invoke (APPENDED) ----
    //
    // Stack at dispatch (bottom → top):
    //   fn_id (native registry index)
    //   args_tuple (Object::Tuple)
    //
    // Operand: low 16 = arity (element count in args tuple).
    // Pops tuple then fn_id, dispatches to the host native
    // registry entry registered via `Machine::register_fn`.
    HostInvoke,
}

impl From<u8> for Instruction {
    fn from(value: u8) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

impl From<Instruction> for u8 {
    fn from(val: Instruction) -> Self {
        val as u8
    }
}


#[derive(Default, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct Byte {
    bytecode: Instruction,
    operands: u32,
    value: u64,
}

impl Byte {
    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            operands: Default::default(),
            value: 0,
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand;

        self
    }

    pub fn with_value_u32(mut self, v: u32) -> Self {
        self.value = v as u64;
        self
    }

    pub fn value_u32(&self) -> u32 {
        self.value as u32
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;

        self.operands = operand;

        self
    }

    pub fn new_with_value(bytecode: Instruction, value: u64) -> Self {
        Self {
            bytecode,
            operands: 0,
            value,
        }
    }

    pub fn bytecode(&self) -> &Instruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands
    }

    ///
    ///```
    /// use common::{Instruction, Byte};
    ///
    /// let mut value: Byte = Byte::new(Instruction::default());
    /// value = value.with_operands_u16([1, 2,]);
    /// assert_eq!(1, value.operand_u16(0));
    /// assert_eq!(2, value.operand_u16(1));
    /// ```
    ///
    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self) -> u64 {
        self.value
    }
}

impl ArchivedByte {
    pub fn new(bytecode: ArchivedInstruction) -> Self {
        Self {
            bytecode,
            operands: Default::default(),
            value: 0.into(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand.into();

        self
    }

    pub fn with_value(mut self, value: Value) -> Self {
        self.value = (value.raw() as u64).into();

        self
    }

    pub fn with_value_u32(mut self, v: u32) -> Self {
        self.value = (v as u64).into();
        self
    }

    pub fn value_u32(&self) -> u32 {
        u64::from(self.value) as u32
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;

        self.operands = operand.into();

        self
    }

    pub fn bytecode(&self) -> &ArchivedInstruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands.into()
    }

    ///
    ///```
    /// use common::{Instruction, Byte};
    ///
    /// let mut value: Byte = Byte::new(Instruction::default());
    /// value = value.with_operands_u16([1, 2,]);
    /// assert_eq!(1, value.operand_u16(0));
    /// assert_eq!(2, value.operand_u16(1));
    /// ```
    ///
    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self) -> u64 {
        self.value.into()
    }
}

#[cfg(debug_assertions)]
impl Debug for Byte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:?}) - {}",
            self.bytecode, self.operands, self.value
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
#[cfg(debug_assertions)]
impl Debug for ArchivedByte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:?}) - {}",
            self.bytecode, self.operands, self.value
        )
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for ArchivedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
