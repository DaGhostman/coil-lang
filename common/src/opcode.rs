use rkyv::{Archive, Deserialize, Serialize};

use crate::Value;

#[repr(u8)]
#[derive(Default, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[cfg_attr(debug_assertions, derive(Debug))]
#[rkyv(compare(PartialEq), derive(Clone), derive(Copy))]
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
    // JumpIfMatch layout (Phase 18C: 32-bit target via constant pool):
    //   operands[31:16] = expected tag (16 bits)
    //   operands[15:0]  = pool index for the 32-bit absolute bytecode target
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

    // ---- Phase VM perf: fused superinstructions (APPENDED) ----
    //
    // These are operator-parameterized: the fused opcode carries the
    // underlying arithmetic/comparison `Instruction` discriminant in
    // the high operand byte, so ONE opcode covers a whole family
    // (`ADD`/`SUB`/`MUL`/`DIV`/`MOD`, all comparisons, float
    // variants, ...) instead of a bespoke opcode per case.
    //
    // `LoadReturnSlot` — fuses `LOAD slot; RETURN` (return a local).
    //   operands[31:0] = slot index (relative to frame.sp)
    //
    // `ConstReturnImm` — fuses `CONST imm; RETURN` (return a small
    // inline constant; pool-backed constants are not fused).
    //   operands[31:0] = the i32 immediate (sign-extended on return)
    //
    // `BinSlotImm` — fuses `LOAD slot; CONST imm; <binop>`, i.e. a
    // binary op between a local and a small inline immediate
    // (`n - 1`, `i + 1`, `x * 2`, `n <= 2`, `k == 0`, ...).
    //   operands[31:24] = op (an integer `Instruction` discriminant:
    //                     ADD/SUB/MUL/DIV/MOD or a comparison)
    //   operands[23:16] = slot index (relative to frame.sp)
    //   operands[15:0]  = signed 16-bit immediate
    //
    // `CmpJmpf` — fuses `<cmp>; JMPF target`: compare the top two
    // stack values and branch when the comparison is FALSE.
    //   operands[31:24] = op (a comparison `Instruction`, int or float)
    //   operands[23:16] = reserved (0)
    //   operands[15:0]  = jump target taken when the compare is false
    //
    // `BinReturn` — fuses `<binop>; RETURN`: apply a binary op to the
    // top two stack values and return the result.
    //   operands[31:24] = op (any binary `Instruction`, int or float)
    //   operands[23:0]  = reserved (0)
    LoadReturnSlot,
    ConstReturnImm,
    BinSlotImm,
    CmpJmpf,
    BinReturn,
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

impl From<u8> for ArchivedInstruction {
    fn from(value: u8) -> Self {
        // `ArchivedInstruction` mirrors `Instruction`'s `#[repr(u8)]`
        // discriminants, so the fused-op operator byte round-trips.
        unsafe { std::mem::transmute(value) }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq))]
pub struct Byte {
    bytecode: Instruction,
    /// Padding keeps the struct at 8 bytes (u8 + 3 pad + u32).
    _pad: [u8; 3],
    operands: u32,
}

impl Byte {
    /// High bit set in a `CONST` operand means the lower 31 bits
    /// index the constant pool.
    pub const POOL_FLAG: u32 = 1 << 31;

    pub fn new(bytecode: Instruction) -> Self {
        Self {
            bytecode,
            _pad: [0; 3],
            operands: Default::default(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand;
        self
    }

    /// Pack folded `CALL`: high 8 bits = arity, low 24 = target.
    pub fn with_call_packed(mut self, arity: u32, target: u32) -> Self {
        debug_assert!(target <= 0xFFFFFF, "CALL target exceeds 24-bit encoding");
        self.operands = (arity << 24) | (target & 0xFFFFFF);
        self
    }

    pub fn call_parts(&self) -> (usize, usize) {
        (
            (self.operands >> 24) as usize,
            (self.operands & 0xFFFFFF) as usize,
        )
    }

    /// Legacy helper — prefer `with_call_packed`.
    pub fn with_value_u32(mut self, v: u32) -> Self {
        if matches!(self.bytecode, Instruction::CALL) {
            let arity = self.operands;
            return self.with_call_packed(arity, v);
        }
        // JumpIfMatch pool index lives in the lower 16 bits; patching
        // replaces the placeholder via `with_operands_u16`.
        self.operands = (self.operands & 0xFFFF_0000) | (v & 0xFFFF);
        self
    }

    pub fn value_u32(&self) -> u32 {
        if matches!(self.bytecode, Instruction::CALL) {
            (self.operands & 0xFFFFFF) as u32
        } else {
            (self.operands & 0xFFFF) as u32
        }
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;
        self.operands = operand;
        self
    }

    /// Emit a `CONST` with an inline i32/bool immediate (no pool).
    pub fn with_const_inline(mut self, value: i32) -> Self {
        debug_assert!(self.bytecode as u8 == Instruction::CONST as u8);
        self.operands = value as u32;
        self
    }

    /// Emit a `CONST` referencing the constant pool at `pool_index`.
    pub fn with_const_pool(mut self, pool_index: u32) -> Self {
        debug_assert!(self.bytecode as u8 == Instruction::CONST as u8);
        self.operands = Self::POOL_FLAG | pool_index;
        self
    }

    pub fn new_with_value(bytecode: Instruction, value: u64) -> Self {
        let v = value as i64;
        if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            Self::new(bytecode).with_const_inline(v as i32)
        } else {
            // Wide values must go through the compiler's pool.
            Self::new(bytecode).with_const_pool(0)
        }
    }

    pub fn bytecode(&self) -> &Instruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands
    }

    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self, pool: &[u64]) -> u64 {
        if self.operands & Self::POOL_FLAG != 0 {
            pool[(self.operands & !Self::POOL_FLAG) as usize]
        } else {
            self.operands as i32 as i64 as u64
        }
    }

    /// Resolve a `JumpIfMatch` target from the constant pool
    /// (index stored in the lower 16 bits of `operands`).
    pub fn jump_if_match_target(&self, pool: &[u64]) -> usize {
        pool[(self.operands & 0xFFFF) as usize] as usize
    }

    /// Pack `BinSlotImm`: op in [31:24], slot in [23:16], signed
    /// immediate in [15:0].
    pub fn with_bin_slot_imm(mut self, op: u8, slot: u8, imm: i16) -> Self {
        self.operands =
            ((op as u32) << 24) | ((slot as u32) << 16) | (imm as u16 as u32);
        self
    }

    /// Unpack `BinSlotImm` into (op, slot, sign-extended immediate).
    pub fn bin_slot_imm_parts(&self) -> (u8, usize, i64) {
        let o = self.operands;
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            (o & 0xFFFF) as u16 as i16 as i64,
        )
    }

    /// Pack `CmpJmpf`: op in [31:24], target in [15:0].
    pub fn with_cmp_jmpf(mut self, op: u8, target: u16) -> Self {
        self.operands = ((op as u32) << 24) | (target as u32);
        self
    }

    /// Unpack `CmpJmpf` into (op, target).
    pub fn cmp_jmpf_parts(&self) -> (u8, usize) {
        (
            (self.operands >> 24) as u8,
            (self.operands & 0xFFFF) as usize,
        )
    }

    /// Pack `BinReturn`: op in [31:24].
    pub fn with_bin_return(mut self, op: u8) -> Self {
        self.operands = (op as u32) << 24;
        self
    }

    /// Unpack the `BinReturn` op.
    pub fn bin_return_op(&self) -> u8 {
        (self.operands >> 24) as u8
    }
}

impl ArchivedByte {
    pub const POOL_FLAG: u32 = Byte::POOL_FLAG;

    pub fn new(bytecode: ArchivedInstruction) -> Self {
        Self {
            bytecode,
            _pad: [0; 3],
            operands: Default::default(),
        }
    }

    pub fn with_operand_u32(mut self, operand: u32) -> Self {
        self.operands = operand.into();
        self
    }

    pub fn with_call_packed(mut self, arity: u32, target: u32) -> Self {
        self.operands = ((arity << 24) | (target & 0xFFFFFF)).into();
        self
    }

    pub fn with_const_inline(mut self, value: i32) -> Self {
        self.operands = (value as u32).into();
        self
    }

    pub fn with_operands_u16(mut self, operands: [u16; 2]) -> Self {
        let mut operand: u32 = 0;
        operand ^= operands[0] as u32;
        operand <<= 16;
        operand ^= operands[1] as u32;
        self.operands = operand.into();
        self
    }

    pub fn with_value(mut self, value: Value) -> Self {
        let raw = value.raw() as u64;
        let v = raw as i64;
        if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
            self.operands = (v as i32 as u32).into();
        } else {
            self.operands = (Self::POOL_FLAG | 0).into();
        }
        self
    }

    pub fn with_value_u32(mut self, v: u32) -> Self {
        self.operands = v.into();
        self
    }

    pub fn value_u32(&self) -> u32 {
        u32::from(self.operands) & 0xFFFFFF
    }

    pub fn bytecode(&self) -> &ArchivedInstruction {
        &self.bytecode
    }

    pub fn operand_u32(&self) -> u32 {
        self.operands.into()
    }

    pub fn operand_u16(&self, index: usize) -> u16 {
        match index {
            0 => (self.operands >> 16) as u16,
            1 => ((self.operands << 16) >> 16) as u16,
            _ => unreachable!("Unable to use larger index when using u32 operands"),
        }
    }

    pub fn constant(&self, pool: &[u64]) -> u64 {
        let op: u32 = self.operands.into();
        if op & Self::POOL_FLAG != 0 {
            pool[(op & !Self::POOL_FLAG) as usize]
        } else {
            op as i32 as i64 as u64
        }
    }

    pub fn call_parts(&self) -> (usize, usize) {
        let op: u32 = self.operands.into();
        ((op >> 24) as usize, (op & 0xFFFFFF) as usize)
    }

    pub fn jump_if_match_target(&self, pool: &[u64]) -> usize {
        let op: u32 = self.operands.into();
        pool[(op & 0xFFFF) as usize] as usize
    }

    /// Unpack `BinSlotImm` into (op, slot, sign-extended immediate).
    pub fn bin_slot_imm_parts(&self) -> (u8, usize, i64) {
        let o: u32 = self.operands.into();
        (
            (o >> 24) as u8,
            ((o >> 16) & 0xFF) as usize,
            (o & 0xFFFF) as u16 as i16 as i64,
        )
    }

    pub fn with_bin_slot_imm(mut self, op: u8, slot: u8, imm: i16) -> Self {
        let packed = ((op as u32) << 24) | ((slot as u32) << 16) | (imm as u16 as u32);
        self.operands = packed.into();
        self
    }

    /// Unpack `CmpJmpf` into (op, target).
    pub fn cmp_jmpf_parts(&self) -> (u8, usize) {
        let o: u32 = self.operands.into();
        ((o >> 24) as u8, (o & 0xFFFF) as usize)
    }

    pub fn with_cmp_jmpf(mut self, op: u8, target: u16) -> Self {
        self.operands = (((op as u32) << 24) | (target as u32)).into();
        self
    }

    /// Unpack the `BinReturn` op.
    pub fn bin_return_op(&self) -> u8 {
        (u32::from(self.operands) >> 24) as u8
    }

    pub fn with_bin_return(mut self, op: u8) -> Self {
        self.operands = ((op as u32) << 24).into();
        self
    }
}

#[cfg(debug_assertions)]
use std::fmt::Debug;

#[cfg(debug_assertions)]
impl Debug for Byte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}({:?})", self.bytecode, self.operands)
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
        write!(f, "{}({:?})", self.bytecode as u8, self.operands)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Display for ArchivedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
