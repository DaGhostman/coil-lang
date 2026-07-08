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

    // ---- Phase 21: register-form opcodes (Dalvik-style hybrid encoding) ----
    //
    // These opcodes are APPENDED (not inserted) to preserve the
    // `#[repr(u8)]` discriminant values of every prior opcode. They
    // are the byte-level instructions for the register VM that
    // replaces the stack VM in Phase 21+. They are NOT consumed by
    // the existing single-pass codegen in `compiler/src/linearize.rs`
    // (which still emits the stack-form opcodes above).
    //
    // ## Operand layout conventions
    //
    // Register indices use a **Dalvik-style hybrid encoding**:
    //
    // - **regs 0..16**: single-byte inline (`operands[7:0]` for the
    //   first register operand, `operands[15:8]` for the second,
    //   `operands[23:16]` for the third, `operands[31:24]` reserved).
    // - **regs 16..256**: `0xFn` high nibble escape + trailing byte
    //   carrying the full register index. The low nibble of the
    //   escape byte is reserved (write 0).
    //
    // Use [`encode_reg`] / [`decode_reg`] to pack and unpack
    // register indices; the helpers handle the escape convention
    // automatically.
    //
    // ## Why a register VM?
    //
    // The stack VM (the existing opcodes above) needs one bytecode
    // per value: `CONST` to push, `LOAD` to push a slot, `STORE` to
    // write back, `DUP`/`POP` to manipulate. Register-form bytecode
    // addresses values by their position in a 256-entry register
    // file, collapsing `DUP/LOAD/STORE` triples into a single `MOV_REG`.
    //
    // ## Phase 21 split
    //
    // - **Phase 21A (this phase)**: opcode vocabulary + Dalvik-style
    //   helpers + rkyv Archived twins. No consumer yet — the legacy
    //   linearizer still emits stack-form bytecode.
    // - **Phase 21B (next)**: register-form emitter (`reg_emit.rs`).
    // - **Phase 21C (next)**: register VM dispatch in `machine/src/vm.rs`.
    // - **Phase 21D (cutover)**: `ARCHIVE_VERSION` bump + deletion of
    //   the stack-form paths.
    //
    // ## Per-opcode operand layouts
    //
    // - `MOV_REG`:    operands[7:0] = dst, operands[15:8] = src.
    // - `MOV_IMM`:    operands[7:0] = dst, operands[31:8] reserved.
    //                 The 64-bit immediate lives in the `value` field.
    // - `ADD_REG`..`MOD_REG`, `ADDF_REG`..`MODF_REG`,
    //   `NEG_REG`, `NOT_REG`, `CMP_REG`, `EQF_REG`..`GEQF_REG`:
    //   operands[7:0] = dst, operands[15:8] = src1, operands[23:16] = src2.
    //                   (Unary ops use operands[7:0] = dst, operands[15:8] = src.)
    // - `JMP_REG`:    operands = absolute bytecode target (full u32;
    //                 no register operands).
    // - `JMPF_REG` / `JMPT_REG`: operands[7:0] = src (the bool to
    //                   test), operands[31:8] reserved. The target
    //                   offset lives in `value[31:0]` (32-bit absolute
    //                   target, mirroring the Phase 18C widening for
    //                   `JumpIfMatch`).
    // - `CALL_REG`:   operands[7:0] = dst reg, operands[15:8] = argc
    //                   (up to 16 inline; values 16..256 use the
    //                   escape byte convention on the next byte), and
    //                   `value[31:0]` carries the **callee name index**
    //                   (the position in the codegen's
    //                   `function_offsets` table — same encoding as
    //                   `CALL`'s `operands` field). The call's
    //                   live-across-call save set is recorded on the
    //                   call site's `Frame::pending_call_save` at
    //                   dispatch time (see Phase 21 mixed calling
    //                   convention docs).
    // - `RET_REG`:    operands[7:0] = src reg (the return value).
    // - `MAKE_ENUM_REG`: operands[7:0] = dst, operands[15:8] = arity,
    //                   operands[31:16] = tag. The payload register
    //                   indices live in trailing bytes (one byte per
    //                   payload, using the inline encoding for regs
    //                   0..16 and the escape form for 16..256).
    // - `JUMP_IF_MATCH_REG`: operands[7:0] = src, operands[31:16] =
    //                   expected tag (16 bits, 65,535 variants ceiling
    //                   matching the existing `JumpIfMatch`). The
    //                   target offset lives in `value[31:0]` (32-bit
    //                   absolute, matching Phase 18C widening).
    // - `UNPACK_REG`: operands[7:0] = src (the enum value), operands[15:8] = arity.
    //                   Trailing bytes carry the destination registers
    //                   (inline for 0..16, escape for 16..256).
    // - `LOAD_FIELD_REG`: operands[7:0] = dst, operands[15:8] = src,
    //                   operands[23:16] = field_index (low 8 bits; the
    //                   high 8 bits of the field index — if > 255 —
    //                   live in the trailing byte convention; see
    //                   `LOAD_FIELD_REG_ESCAPE_BYTE`). For
    //                   field_index 0..256 the inline form suffices.
    // - `PRINT_REG`:  operands[15:0] = argc, operands[31:16] reserved.
    //                   Trailing bytes carry the argument registers.

    // ---- Register moves / immediate constants ----
    /// dst := src (register-to-register copy).
    MovReg,
    /// dst := <imm in `value`> (u64 immediate). The raw bits
    /// follow the same encoding as the existing `CONST` opcode
    /// (i64 raw, f64 raw, bool as 0/1, or heap pointer raw bits).
    MovImm,

    // ---- Integer arithmetic (3-operand: dst, src1, src2) ----
    /// dst := src1 + src2.
    AddReg,
    /// dst := src1 - src2.
    SubReg,
    /// dst := src1 * src2.
    MulReg,
    /// dst := src1 / src2.
    DivReg,
    /// dst := src1 % src2.
    ModReg,
    /// dst := -src (integer negation).
    NegReg,
    /// dst := !src (logical not, on bool).
    NotReg,

    // ---- Float arithmetic ----
    /// dst := src1 +f src2.
    AddFReg,
    /// dst := src1 -f src2.
    SubFReg,
    /// dst := src1 *f src2.
    MulFReg,
    /// dst := src1 /f src2.
    DivFReg,
    /// dst := src1 %f src2.
    ModFReg,

    // ---- Integer comparison (3-operand: dst, src1, src2) ----
    /// dst := (src1 == src2).
    CmpReg,

    // ---- Float comparison (3-operand: dst, src1, src2) ----
    /// dst := (src1 ==f src2).
    EqFReg,
    /// dst := (src1 !=f src2).
    NeqFReg,
    /// dst := (src1 <f  src2).
    LtFReg,
    /// dst := (src1 <=f src2).
    LeqFReg,
    /// dst := (src1 >f  src2).
    GtFReg,
    /// dst := (src1 >=f src2).
    GeqFReg,

    // ---- Control flow ----
    /// Unconditional jump to absolute bytecode target (full u32).
    JmpReg,
    /// Conditional branch: if !src then jump to target.
    /// The target lives in `value[31:0]`.
    JmpfReg,
    /// Conditional branch: if src then jump to target.
    /// The target lives in `value[31:0]`.
    JmptReg,

    // ---- Calls / returns ----
    /// Function call: dst := callee(args). `dst` is unused
    /// (write 0) when the return value is discarded. The
    /// callee name index lives in `value[31:0]`. The args are
    /// passed in regs `r0..rN` (the caller is responsible for
    /// the mixed calling convention: see Phase 21 plan docs).
    CallReg,
    /// Return from current function with `src` as the return value.
    RetReg,

    // ---- Sum types / pattern matching ----
    /// Construct an enum: dst := Enum::Variant(payload).
    /// `tag` and `arity` live in operands[31:16] and operands[15:8];
    /// the payload regs follow as trailing bytes.
    MakeEnumReg,
    /// Multi-way dispatch: if `src` (an enum) has tag ==
    /// `expected_tag`, pop it and push its payload into the
    /// destination registers, then jump to `target`. The target
    /// lives in `value[31:0]` (32-bit absolute). Trailing bytes
    /// carry the destination registers (inline for 0..16, escape
    /// for 16..256).
    JumpIfMatchReg,
    /// Unpack an enum: dst_i := scrutinee.payload[i]. The
    /// destination registers follow as trailing bytes.
    UnpackReg,
    /// Load a record field: dst := src.field_index.
    /// `field_index` lives in operands[23:16].
    LoadFieldReg,

    // ---- I/O ----
    /// Print: side-effecting; pops nothing. The arguments live
    /// in trailing bytes (one register per arg).
    PrintReg,

    // ---- Phase 22b: userland FFI (APPENDED — see append-only contract below) ----
    //
    // CRITICAL: these are appended AFTER every existing variant.
    // Inserting them earlier would shift every later variant's
    // `#[repr(u8)]` discriminant and silently corrupt every `.c0s`
    // archive ever compiled. See `append_only_contract` in the
    // `register_tests` module for the regression guard.
    //
    // `FfiLoad` pops a string (the library path), calls
    // `dlopen`, allocates a heap `Object::Library` wrapping
    // the loaded `Library`, and pushes the library's address
    // as a `Value`. The library's `FunctionSig` table is
    // populated by the host (or by `Machine::register_extern_libs`).
    FfiLoad,
    //
    // `FfiInvoke` resolves a function by ID in the library's
    // signature table, marshals the top `arity` `Value`s into
    // the matched C types, calls the resolved symbol via
    // `libloading`, and pushes the return value (or nothing
    // for `void`). The library object is the value immediately
    // below the args on the stack:
    //
    //   push library_value
    //   push arg_0
    //   ...
    //   push arg_{arity-1}
    //   FfiInvoke function_id, arity
    //
    // The runtime dispatches via the matched C signature's
    // `extern "C" fn(...) -> ...` type, which is one of the
    // six supported by `LibraryFn` (see `machine::ffi`).
    //
    // FfiInvoke (Phase 22b userland API) — takes function_id
    // from the stack (returned by `DeclareFFI`), so the
    // operand only needs to carry arity. See the dispatch
    // arm in `machine/src/vm.rs` for the full stack
    // discipline.
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
    // Stack at dispatch (bottom → top):
    //   lib_handle  name_string  arg_tag_0  arg_tag_1  ...
    //   arg_tag_{arity-1}  ret_type_tag
    //
    // Pops: arity arg_type tags, then the ret_type tag,
    // then the name string, then the lib handle. Resolves
    // the symbol on the library's `Arc<Library>` (via
    // `libloading::Library::get`), builds a `FunctionSig`
    // from the arg/ret tags, registers it on the library's
    // signature table, and pushes the function_id (a fresh
    // `usize` index) for use by FfiInvoke.
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

/// Inline register ceiling for the Dalvik-style hybrid encoding.
///
/// Registers 0..REG_INLINE_MAX use a single inline byte
/// (`operands[7:0]`, `operands[15:8]`, etc.). Registers
/// REG_INLINE_MAX..REG_TOTAL use a 2-byte escape form: the
/// first byte carries `0xF0 | (reg >> 4)` and the trailing byte
/// carries `reg & 0xFF`.
///
/// The 4-bit high-nibble of the escape byte gives a 16x
/// expansion (16 × 16 = 256 register ceiling), which matches
/// the 256-register plan validated by Experiment C
/// (`experiments/regalloc_pressure/`).
pub const REG_INLINE_MAX: u8 = 16;

/// Total register count supported by the encoding. Phase 21
/// pins this at 256 (matching the plan's 4-bit-high-nibble
/// encoding).
pub const REG_TOTAL: u16 = 256;

/// Sentinel byte that signals an escape-form register
/// follows. The low 4 bits encode `reg >> 4` (so the trailing
/// byte carries `reg & 0xFF`).
pub const REG_ESCAPE_PREFIX: u8 = 0xF0;

/// Encode a register index as one or two bytes.
///
/// Returns `(inline_byte, trailing_byte)`:
/// - For `reg < REG_INLINE_MAX`, the inline byte is the
///   register index and the trailing byte is `None`.
/// - For `reg >= REG_INLINE_MAX`, the inline byte is
///   `REG_ESCAPE_PREFIX | (reg >> 4)` (high nibble carries
///   `reg >> 4`) and the trailing byte is `Some(reg & 0xFF)`.
///
/// Callers pack `(inline_byte, trailing_byte)` into the
/// appropriate operand field (`operands[7:0]`, `operands[15:8]`,
/// etc.) and emit the trailing byte as the next bytecode
/// position when `Some`.
pub fn encode_reg(reg: u8) -> (u8, Option<u8>) {
    if reg < REG_INLINE_MAX {
        (reg, None)
    } else {
        let high_nibble = (reg >> 4) & 0x0F;
        let trailing = reg & 0xFF;
        (REG_ESCAPE_PREFIX | high_nibble, Some(trailing))
    }
}

/// Decode a register index from `(inline_byte, trailing_byte)`.
///
/// Inverse of [`encode_reg`]: if the inline byte has the high
/// nibble `0xF`, the second byte is the low 8 bits and the full
/// register index is `(inline & 0x0F) << 4 | (trailing & 0xFF)`
/// (note: the high nibble carries 4 bits, but the trailing byte
/// carries 8 bits, so the low nibble of `inline & 0x0F` would
/// collide with the high nibble of `trailing & 0xFF` for
/// registers ≥ 16 in the same way — see the test suite for the
/// exact reconciliation).
///
/// For `inline < 0xF0`, the register is just `inline` (no
/// trailing byte needed).
pub fn decode_reg(inline: u8, trailing: Option<u8>) -> u8 {
    if inline & 0xF0 == REG_ESCAPE_PREFIX {
        // Escape form: high nibble of inline = reg >> 4,
        // trailing byte = reg & 0xFF (low 8 bits of reg).
        let high = (inline & 0x0F) << 4;
        let low = trailing.unwrap_or(0);
        high | (low & 0x0F)
    } else {
        // Inline form: reg is just the inline byte.
        inline
    }
}

/// Pack three registers into a `u32` operand field using the
/// Dalvik-style encoding. The resulting operand layout is:
///
/// - `operands[7:0]`   = encode_reg(dst_reg).0
/// - `operands[15:8]`  = encode_reg(src1_reg).0
/// - `operands[23:16]` = encode_reg(src2_reg).0
/// - `operands[31:24]` reserved (write 0)
///
/// Returns the packed `u32` operand value AND a `Vec<u8>` of
/// trailing bytes (one per register operand that used the
/// escape form, in source order). The caller emits the trailing
/// bytes as additional bytecode positions immediately after the
/// primary byte.
///
/// For register-only straight-line arithmetic on real
/// workloads (peak live ≤ 4 per Experiment C), the inline form
/// covers every register operand and the trailing Vec is empty.
pub fn encode_3reg(dst: u8, src1: u8, src2: u8) -> (u32, Vec<u8>) {
    let (d_byte, d_trail) = encode_reg(dst);
    let (s1_byte, s1_trail) = encode_reg(src1);
    let (s2_byte, s2_trail) = encode_reg(src2);

    let mut trailing = Vec::new();
    if let Some(b) = d_trail {
        trailing.push(b);
    }
    if let Some(b) = s1_trail {
        trailing.push(b);
    }
    if let Some(b) = s2_trail {
        trailing.push(b);
    }

    let operand: u32 = (d_byte as u32) | ((s1_byte as u32) << 8) | ((s2_byte as u32) << 16);
    (operand, trailing)
}

/// Pack two registers into a `u32` operand field using the
/// Dalvik-style encoding.
///
/// - `operands[7:0]`  = encode_reg(dst_reg).0
/// - `operands[15:8]` = encode_reg(src_reg).0
/// - `operands[31:16]` reserved (write 0)
pub fn encode_2reg(dst: u8, src: u8) -> (u32, Vec<u8>) {
    let (d_byte, d_trail) = encode_reg(dst);
    let (s_byte, s_trail) = encode_reg(src);

    let mut trailing = Vec::new();
    if let Some(b) = d_trail {
        trailing.push(b);
    }
    if let Some(b) = s_trail {
        trailing.push(b);
    }

    let operand: u32 = (d_byte as u32) | ((s_byte as u32) << 8);
    (operand, trailing)
}

/// Pack a single register into a `u32` operand field using the
/// Dalvik-style encoding.
///
/// - `operands[7:0]`  = encode_reg(reg).0
/// - `operands[31:8]` reserved (write 0)
pub fn encode_1reg(reg: u8) -> (u32, Vec<u8>) {
    let (r_byte, r_trail) = encode_reg(reg);
    let operand: u32 = r_byte as u32;
    let trailing = r_trail.into_iter().collect();
    (operand, trailing)
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

#[cfg(test)]
mod register_tests {
    //! Tests for the Phase 21 register-form opcodes and the
    //! Dalvik-style hybrid register encoding helpers.
    //!
    //! These tests verify:
    //! - The new register-form `Instruction` variants have
    //!   distinct, stable `#[repr(u8)]` discriminants (no
    //!   collisions with the existing 0..55 variants).
    //! - `encode_reg` / `decode_reg` round-trip for all 256
    //!   possible register indices.
    //! - `encode_1reg` / `encode_2reg` / `encode_3reg` pack
    //!   registers into the expected u32 operand layout.

    use super::*;

    // =================================================================
    // Phase 21 register-form variants: discriminant stability
    // =================================================================

    #[test]
    fn register_variants_have_distinct_discriminants() {
        // The new variants must each have a unique discriminant.
        let variants = [
            Instruction::MovReg,
            Instruction::MovImm,
            Instruction::AddReg,
            Instruction::SubReg,
            Instruction::MulReg,
            Instruction::DivReg,
            Instruction::ModReg,
            Instruction::NegReg,
            Instruction::NotReg,
            Instruction::AddFReg,
            Instruction::SubFReg,
            Instruction::MulFReg,
            Instruction::DivFReg,
            Instruction::ModFReg,
            Instruction::CmpReg,
            Instruction::EqFReg,
            Instruction::NeqFReg,
            Instruction::LtFReg,
            Instruction::LeqFReg,
            Instruction::GtFReg,
            Instruction::GeqFReg,
            Instruction::JmpReg,
            Instruction::JmpfReg,
            Instruction::JmptReg,
            Instruction::CallReg,
            Instruction::RetReg,
            Instruction::MakeEnumReg,
            Instruction::JumpIfMatchReg,
            Instruction::UnpackReg,
            Instruction::LoadFieldReg,
            Instruction::PrintReg,
        ];
        // `Instruction` is `Eq + Copy` but not `Hash`, so we
        // deduplicate via a Vec rather than a HashSet.
        let mut seen: Vec<u8> = Vec::new();
        for v in &variants {
            let d = *v as u8;
            assert!(
                !seen.contains(&d),
                "duplicate discriminant {} for {:?}",
                d,
                v
            );
            seen.push(d);
        }
        assert_eq!(variants.len(), 31, "expected 31 new variants");
    }

    #[test]
    fn register_variants_are_appended_after_existing_55() {
        // Every new variant's discriminant must be > 54 (the
        // previous maximum, which was UnpackAt). This is the
        // discriminant-stability guarantee — inserting a
        // variant before UnpackAt would shift every later
        // discriminant and silently corrupt every archived
        // `.c0s` file.
        let new_variants = [
            Instruction::MovReg,
            Instruction::MovImm,
            Instruction::AddReg,
            Instruction::JmpReg,
            Instruction::CallReg,
            Instruction::PrintReg,
            Instruction::JumpIfMatchReg,
            Instruction::UnpackReg,
            Instruction::LoadFieldReg,
        ];
        for v in &new_variants {
            let d = *v as u8;
            assert!(
                d > 54,
                "variant {:?} has discriminant {} which is not > 54 (UnpackAt's value)",
                v,
                d
            );
        }
    }

    #[test]
    fn register_variants_appended_in_order() {
        // Source order matches discriminant order — appended
        // variants 55, 56, 57, ...
        assert_eq!(Instruction::MovReg as u8, 55);
        assert_eq!(Instruction::MovImm as u8, 56);
        assert_eq!(Instruction::AddReg as u8, 57);
        assert_eq!(Instruction::SubReg as u8, 58);
        assert_eq!(Instruction::MulReg as u8, 59);
        assert_eq!(Instruction::DivReg as u8, 60);
        assert_eq!(Instruction::ModReg as u8, 61);
        assert_eq!(Instruction::NegReg as u8, 62);
        assert_eq!(Instruction::NotReg as u8, 63);
        assert_eq!(Instruction::AddFReg as u8, 64);
        assert_eq!(Instruction::SubFReg as u8, 65);
        assert_eq!(Instruction::MulFReg as u8, 66);
        assert_eq!(Instruction::DivFReg as u8, 67);
        assert_eq!(Instruction::ModFReg as u8, 68);
        assert_eq!(Instruction::CmpReg as u8, 69);
        assert_eq!(Instruction::EqFReg as u8, 70);
        assert_eq!(Instruction::NeqFReg as u8, 71);
        assert_eq!(Instruction::LtFReg as u8, 72);
        assert_eq!(Instruction::LeqFReg as u8, 73);
        assert_eq!(Instruction::GtFReg as u8, 74);
        assert_eq!(Instruction::GeqFReg as u8, 75);
        assert_eq!(Instruction::JmpReg as u8, 76);
        assert_eq!(Instruction::JmpfReg as u8, 77);
        assert_eq!(Instruction::JmptReg as u8, 78);
        assert_eq!(Instruction::CallReg as u8, 79);
        assert_eq!(Instruction::RetReg as u8, 80);
        assert_eq!(Instruction::MakeEnumReg as u8, 81);
        assert_eq!(Instruction::JumpIfMatchReg as u8, 82);
        assert_eq!(Instruction::UnpackReg as u8, 83);
        assert_eq!(Instruction::LoadFieldReg as u8, 84);
        assert_eq!(Instruction::PrintReg as u8, 85);
    }

    // =================================================================
    // Dalvik-style register encoding: encode_reg / decode_reg
    // =================================================================

    #[test]
    fn encode_reg_inline_for_regs_0_through_15() {
        for reg in 0..REG_INLINE_MAX {
            let (inline, trailing) = encode_reg(reg);
            assert_eq!(inline, reg, "reg {} should be inline byte {}", reg, reg);
            assert!(
                trailing.is_none(),
                "reg {} should have no trailing byte",
                reg
            );
        }
    }

    #[test]
    fn encode_reg_escape_form_for_regs_16_through_255() {
        for reg in REG_INLINE_MAX..=255u8 {
            let (inline, trailing) = encode_reg(reg);
            // High nibble must be 0xF (the escape prefix).
            assert_eq!(
                inline & 0xF0,
                REG_ESCAPE_PREFIX,
                "reg {} should use escape prefix 0xF0",
                reg
            );
            // The low nibble encodes reg >> 4 (high 4 bits of reg).
            assert_eq!(
                inline & 0x0F,
                (reg >> 4) & 0x0F,
                "reg {} low nibble mismatch",
                reg
            );
            // Trailing byte carries reg & 0xFF.
            assert_eq!(trailing, Some(reg), "reg {} trailing byte mismatch", reg);
        }
    }

    #[test]
    fn decode_reg_inverts_encode_reg_for_all_256_values() {
        // For each register index 0..=255, encode then decode
        // must round-trip the value. This is the canonical
        // regression test for the encoding helpers.
        for reg in 0u32..=255 {
            let reg_u8 = reg as u8;
            let (inline, trailing) = encode_reg(reg_u8);
            let decoded = decode_reg(inline, trailing);
            assert_eq!(
                decoded, reg_u8,
                "round-trip failed for reg {} (inline=0x{:02X}, trailing={:?})",
                reg, inline, trailing
            );
        }
    }

    #[test]
    fn decode_reg_inline_form_treats_byte_as_register() {
        // Inline form (no escape prefix): the byte IS the
        // register index.
        assert_eq!(decode_reg(0, None), 0);
        assert_eq!(decode_reg(5, None), 5);
        assert_eq!(decode_reg(15, None), 15);
        // Trailing byte is ignored when not in escape form.
        assert_eq!(decode_reg(7, Some(99)), 7);
    }

    #[test]
    fn encode_reg_handles_boundary_reg_15_and_16() {
        // Reg 15 is the last inline register.
        let (inline, trailing) = encode_reg(15);
        assert_eq!(inline, 15);
        assert!(trailing.is_none());
        // Reg 16 is the first escape-form register.
        let (inline, trailing) = encode_reg(16);
        assert_eq!(inline, REG_ESCAPE_PREFIX | 0x01);
        assert_eq!(trailing, Some(16));
    }

    #[test]
    fn encode_reg_handles_high_registers() {
        // Reg 255 should use the full 0xFF high nibble +
        // 0xFF trailing byte.
        let (inline, trailing) = encode_reg(255);
        assert_eq!(inline, REG_ESCAPE_PREFIX | 0x0F);
        assert_eq!(trailing, Some(255));
    }

    // =================================================================
    // Packed operand encoders
    // =================================================================

    #[test]
    fn encode_3reg_packs_three_inline_registers() {
        // Three small regs (all < 16) pack into the low 24
        // bits of the operand; trailing bytes empty.
        let (operand, trailing) = encode_3reg(3, 7, 12);
        assert_eq!(trailing, Vec::<u8>::new());
        assert_eq!(operand & 0xFF, 3, "dst in low 8 bits");
        assert_eq!((operand >> 8) & 0xFF, 7, "src1 in bits 8..15");
        assert_eq!((operand >> 16) & 0xFF, 12, "src2 in bits 16..23");
        assert_eq!(operand >> 24, 0, "high byte reserved");
    }

    #[test]
    fn encode_3reg_packs_escape_form_registers_with_trailing_bytes() {
        // Reg 20 and 30 use escape form; the trailing bytes
        // appear in source order (dst first, then src1, then src2).
        let (operand, trailing) = encode_3reg(20, 30, 5);
        assert_eq!(
            trailing,
            vec![20, 30],
            "trailing bytes appear in src1, src2 order for escape regs"
        );
        assert_eq!(operand & 0xFF, u32::from(REG_ESCAPE_PREFIX | (20 >> 4)));
        assert_eq!(
            (operand >> 8) & 0xFF,
            u32::from(REG_ESCAPE_PREFIX | (30 >> 4))
        );
        assert_eq!((operand >> 16) & 0xFF, 5u32);
    }

    #[test]
    fn encode_2reg_packs_two_inline_registers() {
        let (operand, trailing) = encode_2reg(4, 11);
        assert_eq!(trailing, Vec::<u8>::new());
        assert_eq!(operand & 0xFF, 4);
        assert_eq!((operand >> 8) & 0xFF, 11);
        assert_eq!(operand >> 16, 0);
    }

    #[test]
    fn encode_1reg_packs_inline_register() {
        let (operand, trailing) = encode_1reg(9);
        assert_eq!(trailing, Vec::<u8>::new());
        assert_eq!(operand, 9);
    }

    #[test]
    fn encode_1reg_packs_escape_register() {
        let (operand, trailing) = encode_1reg(100);
        assert_eq!(trailing, vec![100]);
        assert_eq!(operand & 0xFF, u32::from(REG_ESCAPE_PREFIX | (100 >> 4)));
    }

    // =================================================================
    // Byte-level integration: register variants produce valid bytes
    // =================================================================

    #[test]
    fn mov_reg_byte_carries_dst_and_src_in_operands() {
        // MOV_REG dst=2 src=5 → operands[7:0]=2, operands[15:8]=5.
        let (operand, trailing) = encode_2reg(2, 5);
        assert!(trailing.is_empty());
        let byte = Byte::new(Instruction::MovReg).with_operand_u32(operand);
        assert_eq!(*byte.bytecode(), Instruction::MovReg);
        assert_eq!(byte.operand_u32(), operand);
    }

    #[test]
    fn mov_imm_byte_carries_immediate_in_value_field() {
        // MOV_IMM dst=0 imm=42 → operands low byte is dst
        // (REG_INLINE_MAX allows 0..16 inline), value is 42.
        let (operand, trailing) = encode_1reg(0);
        assert!(trailing.is_empty());
        let byte = Byte::new(Instruction::MovImm)
            .with_operand_u32(operand)
            .with_value_u32(42);
        assert_eq!(*byte.bytecode(), Instruction::MovImm);
        assert_eq!(byte.value_u32(), 42);
    }

    #[test]
    fn jump_if_match_reg_byte_carries_tag_and_wide_target() {
        // JUMP_IF_MATCH_REG src=3 tag=5 target=1000
        // → operands[7:0]=3, operands[31:16]=5, value[31:0]=1000.
        //
        // `operand_u16(0)` reads the upper 16 bits (the tag
        // in our encoding); `operand_u16(1)` reads the lower
        // 16 bits (the register index in the low 8 bits, zero
        // in the next 8).
        let (operand, trailing) = encode_1reg(3);
        assert!(trailing.is_empty());
        let byte = Byte::new(Instruction::JumpIfMatchReg)
            .with_operand_u32(operand | (5u32 << 16))
            .with_value_u32(1000);
        assert_eq!(byte.operand_u16(0), 5, "tag lives in upper 16 bits");
        assert_eq!(byte.operand_u16(1), 3, "src reg lives in lower 16 bits");
        assert_eq!(byte.value_u32(), 1000, "target lives in value[31:0]");
    }
}
