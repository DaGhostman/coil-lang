//! Linearizer: convert a CFG `Function` into stack-based bytecode.
//!
//! Phase 1.3 scope: multi-block CFG linearization with full
//! control-flow support (`Jump`, `Branch`, `Switch`).
//!
//! See [`MULTI_PASS_REFACTOR_PLAN.md`](../MULTI_PASS_REFACTOR_PLAN.md)
//! §3 for the high-level design.
//!
//! # What this linearizer does
//!
//! Walks a CFG [`Function`] block-by-block in declaration order
//! and emits one [`common::Byte`] per instruction. Block-local
//! terminators (`Jump`, `Branch`, `Switch`) are emitted as
//! placeholders with operand `0` (and `value[31:0] = 0` for
//! `Switch`'s wide target) and patched in a second pass once
//! every block's offset is known.
//!
//! The emitted bytecode is the existing stack-based form (no
//! register allocator yet — that's Phase 3).
//!
//! # Algorithm (two-pass declaration-order linearization)
//!
//! 1. **Pass 1 — emit**: walk `cfg.blocks` in declaration order
//!    (index 0, 1, 2, ...). For each block, emit its
//!    instructions, then emit a placeholder for its terminator.
//!    Record every block's offset (start byte position) in
//!    `block_offsets`. For terminators that need patching
//!    (`Jump`, `Branch`, `Switch`), record the placeholder's byte
//!    position and its target [`BlockId`] for the second pass.
//! 2. **Pass 2 — patch**: walk the recorded patches and write
//!    the actual target offset (`block_offsets[target.index()]`
//!    as `u32`) into each placeholder's operand (or `value[31:0]`
//!    for `Switch`'s wide target).
//!
//! Declaration order is sufficient for straight-line if-else
//! (no loops, no back-edges). RPO will be needed for Phase 1.4
//! (loops) when back-edges become possible.
//!
//! # Branch terminator layout
//!
//! `Terminator::Branch { true_bb, false_bb, .. }` emits a single
//! `JMPF` placeholder. The `JMPF` jumps to `false_bb` when the
//! condition is false; when true, control falls through to the
//! NEXT block in declaration order. This means the block layout
//! must put the "true" arm immediately after the Branch's
//! block. The [`crate::cfg_builder::Builder::build_if`] method
//! already produces this layout (entry → join → then →
//! false_target, where the Branch's true_bb is `then` and the
//! next declaration-order block IS `then`).
//!
//! # Switch terminator layout (Phase 1.3)
//!
//! `Terminator::Switch { scrutinee, cases, default }` emits a
//! cascade of `JUMP_IF_MATCH` placeholders (one per case) plus
//! a single trailing `UNPACK` for the default arm. The
//! `JUMP_IF_MATCH` jumps to its target arm block on a tag
//! match; on a miss it falls through to the next
//! `JUMP_IF_MATCH` (or to the `UNPACK` after the last case).
//!
//! Layout for `match x { A => body_a, B => body_b, C => body_c }`
//! with 3 arms (2 cases + default):
//!
//! ```text
//! [match_block]      JUMP_IF_MATCH tag_A → arm_a_block
//!                    JUMP_IF_MATCH tag_B → arm_b_block
//!                    UNPACK arity_C         (default arm — fall-through)
//! [arm_a_block]      body_a
//!                    JMP join_block
//! [arm_b_block]      body_b
//!                    JMP join_block
//! [arm_c_block]      body_c   ← reached by UNPACK fall-through
//!                    JMP join_block
//! [join_block]       ...
//! ```
//!
//! This is the **simplified forward emission** layout. The
//! existing single-pass codegen (Phase 15C) uses a more
//! efficient **reverse-source-order** layout (arm bodies
//! emitted in reverse, then the JUMP_IF_MATCH cascade after
//! them). For Phase 1.3 we use forward emission — simpler to
//! implement and correct; the optimization to reverse
//! emission is a future enhancement.
//!
//! The `JUMP_IF_MATCH` opcode's tag is encoded in
//! `operands[31:16]` (16 bits) and the target offset is in
//! `value[31:0]` (32 bits, after Phase 18C's widening). The
//! target is wide enough for any realistic match arm body.
//!
//! # What this linearizer does NOT do
//!
//! - **SSA value tracking.** The linearizer does NOT track which
//!   stack slot holds which SSA [`ValueId`]. For straight-line
//!   code, this works because each value is produced and
//!   immediately consumed in source order (the stack-top
//!   invariant). Phase 3 will add real register allocation.
//!
//! - **Constructor pattern binding code.** The `cfg_builder`
//!   accepts `Some(v) => ...`-style patterns but does NOT emit
//!   `STORE` / `POP` instructions to bind `v` from the
//!   scrutinee payload. Phase 1.4+ will add the binding code.
//!
//! - **Call target resolution.** Function call targets are
//!   resolved by the existing pipeline (see `compiler/src/lib.rs`),
//!   not here. The linearizer emits a `JMP u32::MAX` placeholder
//!   that the upstream patch step fills in.
//!
//! - **Reverse Post-Order (RPO) block ordering.** Declaration
//!   order is fine for if-else and Switch without back-edges.
//!   Phase 1.4 (loops) will need RPO.
//!
//! # Operand conventions
//!
//! [`common::Byte`] has two operand fields:
//! - `operands: u32` — small immediates (slot offsets, arities,
//!   tags, jump targets). Set via [`common::Byte::with_operand_u32`].
//! - `value: u64` — full-width immediates (i64/f64 constants
//!   AND wide `JUMP_IF_MATCH` targets). Set via
//!   [`common::Byte::new_with_value`] or
//!   [`common::Byte::with_value_u32`].
//!
//! Constants (`Inst::Const`, `Inst::ConstF`, `Inst::ConstBool`)
//! use `value`; everything else uses `operands`. Jump targets
//! for `JMP` / `JMPF` use `operands`; the `JUMP_IF_MATCH`
//! target uses `value[31:0]` (Phase 18C's wide target).

use common::{Byte, Instruction, Value};

use crate::cfg::{BinOpKind, Function, Inst, Terminator, UnaryOpKind};

/// Linearize a CFG [`Function`] into stack-based bytecode.
///
/// `base_offset` is the offset where this function's bytecode
/// will be placed in the program. All jump targets (`JMP`,
/// `JMPF`, `JUMP_IF_MATCH`) are computed relative to the start
/// of the function's bytecode, then `base_offset` is added so
/// they are **absolute** offsets in the program. The VM reads
/// jump operands as absolute offsets, so without this addition
/// jumps would land in the wrong place when the function is
/// appended at a non-zero program offset.
///
/// Pass `0` for `base_offset` if the linearized bytecode will
/// be placed at the start of the program (or in isolation, e.g.
/// in unit tests).
///
/// # Phase 1.3 scope
///
/// - **Multi-block CFGs** (control flow with `Jump`, `Branch`,
///   and `Switch` terminators) are supported via declaration-
///   order emission + back-patching. See module docs for the
///   algorithm and the `Switch` layout.
/// - **Sequential instruction emission.** SSA values are NOT
///   tracked — the linearizer assumes each value is at the
///   expected stack position (stack-top invariant for
///   straight-line code and for the single straight-line
///   segments within each block).
/// - **Call target is a placeholder.** `JMP u32::MAX`; the
///   upstream pipeline patches it after linearization.
///
/// # Returns
///
/// A `Vec<Byte>` of bytecode instructions. The vector is empty
/// for a function with no blocks (defensive — well-formed CFGs
/// always have at least the entry block).
///
/// `#[allow(dead_code)]` — this module is wired into the
/// pipeline via `try_compile_function_via_cfg`; until the CFG
/// path is exercised by the test suite for control-flow
/// functions, Rust's dead-code lint would otherwise flag the
/// entry point and its helpers.
#[allow(dead_code)]
pub fn linearize(cfg: &Function, base_offset: u32) -> Vec<Byte> {
    let mut bytecode = Vec::new();
    let mut block_offsets: Vec<usize> = Vec::with_capacity(cfg.blocks.len());
    let mut patches: Vec<TerminatorPatch> = Vec::new();

    // Pass 1: walk blocks in declaration order, emit
    // instructions and terminator placeholders. Track each
    // block's offset RELATIVE TO THE START OF THIS FUNCTION'S
    // BYTECODE (i.e., relative to `base_offset`). The actual
    // jump operand is `base_offset + block_offsets[i]`.
    for block in &cfg.blocks {
        block_offsets.push(bytecode.len());

        // Emit straight-line instructions.
        for inst in &block.insts {
            emit_inst(inst, &mut bytecode);
        }

        // Emit terminator placeholder; record patch positions
        // for terminators that need them.
        emit_terminator_placeholder(&block.terminator, &mut bytecode, &mut patches);
    }

    // Pass 2: patch terminator placeholders with the actual
    // absolute target byte offsets. Block offsets are now
    // stable (no more bytecode emission), so
    // `base_offset + block_offsets[target.index()]` is the
    // correct absolute offset in the program bytecode.
    for patch in &patches {
        patch_terminator(patch, &block_offsets, base_offset, &mut bytecode);
    }

    bytecode
}

/// Emit a single CFG [`Inst`] as one or more bytecode instructions.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn emit_inst(inst: &Inst, bc: &mut Vec<Byte>) {
    match inst {
        Inst::Const { dst: _, value } => {
            // CONST with full 64-bit immediate. Round-trip via
            // `Value::from` so the encoding matches the existing
            // codegen path (which uses `Value::from(*num).raw() as _`).
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstF { dst: _, value } => {
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstBool { dst: _, value } => {
            bc.push(Byte::new_with_value(
                Instruction::CONST,
                Value::from(*value).raw() as u64,
            ));
        }
        Inst::ConstString { dst: _, value } => {
            // STRING + DATA sequence: emit STRING first, then the
            // DATA chars that STRING will read at runtime.
            //
            // The VM's `Instruction::STRING` handler reads the
            // NEXT `length` bytes via `code.next()` and pushes
            // a string onto the stack. The bytes it reads are
            // expected to be `DATA` instructions. So the
            // bytecode order is:
            //
            //   [N]    STRING length
            //   [N+1]  DATA char_0
            //   [N+2]  DATA char_1
            //   ...
            //   [N+length] DATA char_(length-1)
            //
            // This matches the existing single-pass codegen
            // (see `compiler/src/lib.rs::Expression::String`,
            // which inserts STRING at idx and then pushes DATA
            // chars after it).
            //
            // Phase 1.6: the pre-Phase-1.6 linearizer emitted
            // the bytes in the OPPOSITE order (DATA chars first,
            // then STRING) — this passed the unit test (which
            // only checked byte shape, not runtime behavior) but
            // BROKE at runtime: STRING would read the following
            // instruction's bytes (e.g., PRINT, RETURN) as DATA
            // chars, silently producing a corrupted string.
            // Exposed by `examples/print_literal.0s`, which is
            // the first CFG-path example to use a string literal.
            let chars: Vec<char> = value.chars().collect();
            let count = chars.len() as u32;
            bc.push(Byte::new(Instruction::STRING).with_operand_u32(count));
            for c in chars {
                bc.push(Byte::new(Instruction::DATA).with_operand_u32(c as u32));
            }
        }
        Inst::Param { dst: _, index } => {
            // LOAD reads from `stack[frame.sp + operand]`. The
            // parameter at source position `index` lives at
            // `stack[frame.sp + index]`, so the operand is the
            // index directly.
            bc.push(Byte::new(Instruction::LOAD).with_operand_u32(*index as u32));
        }
        Inst::BinOp {
            op,
            dst: _,
            lhs: _,
            rhs: _,
        } => {
            // The operands `lhs` and `rhs` are SSA [`ValueId`]s;
            // for straight-line code the values are still on the
            // stack at the expected positions (stack-top invariant).
            // The linearizer does NOT track SSA values (Phase 1+).
            // We just emit the operation.
            bc.push(map_binop(*op));
        }
        Inst::UnaryOp { op, dst: _, src: _ } => {
            bc.push(map_unaryop(*op));
        }
        Inst::Call {
            dst,
            callee: _,
            args,
        } => {
            // CALL arity + JMP target. The target is a placeholder
            // (u32::MAX) — the upstream pipeline patches it after
            // linearization (see `compiler/src/lib.rs` for the
            // existing patch path).
            //
            // CALL pushes the result (if any) onto the stack; we
            // don't emit anything for the result — the VM does it.
            bc.push(Byte::new(Instruction::CALL).with_operand_u32(args.len() as u32));
            bc.push(Byte::new(Instruction::JMP).with_operand_u32(u32::MAX));
            // The `dst` SSA value lives on the stack after CALL
            // returns. The linearizer doesn't track it (Phase 1+).
            let _ = dst;
        }
        Inst::LoadField {
            dst: _,
            src: _,
            field_index,
        } => {
            // LoadField pops the receiver (an `Object::Enum`) and
            // pushes `payload[field_index]`. The receiver is
            // assumed to be at the stack top (stack-top invariant).
            //
            // Layout (see `common/src/opcode.rs`):
            //   operands[15:0]  = field_index
            //   operands[31:16] = reserved
            bc.push(Byte::new(Instruction::LoadField).with_operand_u32(*field_index as u32));
        }
        Inst::Unpack {
            dst: _,
            scrutinee: _,
        } => {
            // UNPACK pops the scrutinee and pushes the payload
            // values. The arity is `dst.len()`.
            //
            // For Phase 0.3a, the linearizer only emits this for
            // single-block functions where the unpack is followed
            // by a RETURN — so the actual UNPACK instruction is
            // not strictly necessary (we could just return the
            // scrutinee directly). We emit it for fidelity with
            // the spec; the resulting bytecode is a no-op in the
            // simplest cases.
            //
            // Phase 1 will wire this properly for multi-block
            // match arms.
            bc.push(Byte::new(Instruction::Unpack).with_operand_u32(0));
        }
        Inst::MakeEnum {
            dst: _,
            tag,
            payload,
        } => {
            // MAKE_ENUM pops `payload.len()` values and pushes the
            // enum. The values are at the stack top in REVERSE
            // declaration order (per the codegen convention — see
            // Phase 15C's reverse-emit decision).
            //
            // Layout (see `common/src/opcode.rs`):
            //   operands[31:16] = tag
            //   operands[15:0]  = arity
            let arity = (payload.len() as u32) & 0xFFFF;
            let tag_shifted = (*tag & 0xFFFF) << 16;
            bc.push(Byte::new(Instruction::MakeEnum).with_operand_u32(tag_shifted | arity));
        }
        Inst::Print { args } => {
            // PRINT pops the format string (and any params in
            // the format-specifier case) from the stack and
            // prints the formatted output.
            //
            // Two cases:
            //
            //   1. `args.len() == 1` — `print "literal";`. The
            //      stack top holds the format string. Emit a
            //      single `PRINT` opcode (no operands). This is
            //      the simple case from the previous Phase 1.6
            //      fix.
            //
            //   2. `args.len() > 1` — `print "%i", x;`. The
            //      stack holds (top to bottom): `param_0,
            //      param_1, ..., param_(N-1), format_string`.
            //      The cfg_builder pushes the format string
            //      FIRST (via `ConstString`) and the params
            //      AFTER (in source order), so the resulting
            //      stack matches the single-pass codegen's
            //      layout exactly: params stacked on top of the
            //      format string. Emit `FORMAT(N)` to pop the
            //      N+1 values, then `PRINT` to print the
            //      resulting formatted string.
            //
            // Format specifiers in the format string consume
            // params in REVERSE source order (this is the
            // existing single-pass codegen's quirk — see
            // `machine/src/vm.rs`'s `Instruction::FORMAT`
            // dispatch: `params.pop()` returns the LAST-pushed
            // param first). For the common `%i` with one arg
            // case this is fine. Multi-arg / multi-specifier
            // cases would need a `format!`-style API to
            // reverse the specifiers; deferred to a future
            // phase.
            if args.len() == 1 {
                // Simple case: no format specifiers, just a
                // literal format string. Emit a single PRINT.
                bc.push(Byte::new(Instruction::PRINT));
            } else {
                // Format-specifier case: emit FORMAT with
                // `params_count = args.len() - 1` (args[0] is
                // the format string, args[1..] are the
                // params), then PRINT.
                let params_count = (args.len() - 1) as u32;
                bc.push(Byte::new(Instruction::FORMAT).with_operand_u32(params_count));
                bc.push(Byte::new(Instruction::PRINT));
            }
        }
    }
}

/// Emit a CFG [`Terminator`] as a placeholder, and record any
/// patch position for the second pass.
///
/// Terminators that encode a jump target (`Jump`, `Branch`,
/// `Switch`) emit a placeholder with operand `0` (and
/// `value[31:0] = 0` for `Switch`'s wide target) and append
/// one or more [`TerminatorPatch`]es to `patches` so the second
/// pass can fill in the real target offset. Terminators with
/// no jump target (`Return`, `Unreachable`) emit their final
/// bytecode directly.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn emit_terminator_placeholder(
    term: &Terminator,
    bc: &mut Vec<Byte>,
    patches: &mut Vec<TerminatorPatch>,
) {
    match term {
        Terminator::Jump(target) => {
            // Unconditional jump. Emit JMP placeholder and
            // record the patch.
            let pos = bc.len();
            bc.push(Byte::new(Instruction::JMP).with_operand_u32(0));
            patches.push(TerminatorPatch {
                kind: PatchKind::Jump(*target),
                pos,
            });
        }
        Terminator::Branch {
            cond: _,
            true_bb: _,
            false_bb,
        } => {
            // Conditional branch. We emit a single `JMPF`
            // placeholder that, when patched, jumps to
            // `false_bb`. The `true_bb` is reached by
            // FALL-THROUGH from the JMPF — i.e., the block
            // immediately following the Branch's block in
            // declaration order must be `true_bb`'s block (this
            // is what `cfg_builder::build_if` produces).
            let pos = bc.len();
            bc.push(Byte::new(Instruction::JMPF).with_operand_u32(0));
            patches.push(TerminatorPatch {
                kind: PatchKind::Branch {
                    false_bb: *false_bb,
                },
                pos,
            });
        }
        Terminator::Return(ret) => {
            // Unit (void) return — push a default value (0) before
            // RETURN so the VM has something to pop. The existing
            // single-pass codegen does the same (see
            // `compiler/src/lib.rs::Expression::Function` post-
            // processing around `if !matches!(... Instruction::RETURN)`).
            //
            // Phase 1.6: this fix is required for void functions
            // (e.g., `print "hello";` without an explicit
            // `return ...;`) to round-trip through the CFG path.
            // Before this fix, the linearizer emitted just
            // `RETURN`, leaving the stack empty — the VM's
            // `ExecutionOutcome::RETURN` handler would panic with
            // `assertion failed: self.cursor > 0` in
            // `machine/src/memory/stack.rs`.
            //
            // Value-returning functions have the return value
            // already on the stack (pushed by the preceding
            // `Inst`), so we just emit `RETURN`.
            //
            // Phase 1.6 caveat: the cfg_builder's Identifier-of-
            // param fast path (see `cfg_builder.rs::Expression::Return`)
            // also uses `Return(None)` for value-returning
            // functions — but it pushes an `Inst::Param` first,
            // which loads the value onto the stack. We detect
            // this case by checking the LAST emitted instruction:
            // if it's `LOAD` (the bytecode for `Inst::Param`),
            // the value is on the stack and we don't push CONST 0.
            // Otherwise (void function, or print-only function
            // where the last inst consumed the stack top), we
            // push CONST 0 to give the VM something to pop.
            match ret {
                Some(_) => {
                    // Value-returning function with explicit
                    // value — the value is already on the stack.
                    bc.push(Byte::new(Instruction::RETURN));
                }
                None => {
                    let last_is_param_load =
                        matches!(bc.last().map(|b| b.bytecode()), Some(Instruction::LOAD));
                    if last_is_param_load {
                        // Phase 1.6 fast path: the preceding
                        // Param pushed the value. RETURN pops it.
                        bc.push(Byte::new(Instruction::RETURN));
                    } else {
                        // Void return — push a default value
                        // (CONST 0) so RETURN has something to pop.
                        bc.push(Byte::new_with_value(
                            Instruction::CONST,
                            Value::default().raw() as _,
                        ));
                        bc.push(Byte::new(Instruction::RETURN));
                    }
                }
            }
        }
        Terminator::Unreachable => {
            // `HALT` is the canonical "unreachable" terminator
            // (matches the prologue pattern in
            // `compiler/src/lib.rs::Default for Compiler`).
            bc.push(Byte::new(Instruction::HALT));
        }
        Terminator::Switch {
            scrutinee: _,
            cases,
            default: _,
        } => {
            // Phase 1.3: Switch terminator linearization.
            //
            // Emit a JUMP_IF_MATCH placeholder for each case
            // (in `cases` order) and a single trailing UNPACK
            // for the default arm. Each JUMP_IF_MATCH
            // placeholder records its own patch entry (with
            // tag and target BlockId); the UNPACK doesn't
            // need patching — it falls through to the NEXT
            // block in declaration order, which is the
            // default arm block (the cfg_builder pushes arm
            // blocks in source order so the LAST arm is the
            // default and comes immediately after the
            // match_block).
            //
            // JumpIfMatch layout (see `common/src/opcode.rs`,
            // Phase 18C):
            //   operands[31:16] = expected tag (16 bits)
            //   operands[15:0]  = reserved (write 0)
            //   value[31:0]     = absolute bytecode target
            //
            // We mask the tag to 16 bits so a malformed
            // placeholder doesn't silently truncate.
            for (tag, target) in cases {
                let pos = bc.len();
                let byte = Byte::new(Instruction::JumpIfMatch)
                    .with_operand_u32((*tag & 0xFFFF) << 16)
                    .with_value_u32(0);
                bc.push(byte);
                patches.push(TerminatorPatch {
                    kind: PatchKind::SwitchCase {
                        tag: *tag,
                        target: *target,
                    },
                    pos,
                });
            }
            // Emit UNPACK for the default arm. The arity is a
            // placeholder (the VM reads the real arity from
            // `ObjEnum::payload.len()` at runtime, so 0 is
            // safe).
            bc.push(Byte::new(Instruction::Unpack).with_operand_u32(0));
        }
    }
}

/// Patch a single terminator placeholder with its real target
/// offset, looked up from `block_offsets` and added to
/// `base_offset` to produce the **absolute** program-bytecode
/// offset.
///
/// `Jump`, `Branch`, and `SwitchCase` placeholders are recorded
/// (see [`emit_terminator_placeholder`]); `Return` and
/// `Unreachable` emit their final form directly and never
/// appear in the patches list. Each `Switch` case gets its own
/// `SwitchCase` patch entry.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn patch_terminator(
    patch: &TerminatorPatch,
    block_offsets: &[usize],
    base_offset: u32,
    bc: &mut Vec<Byte>,
) {
    match patch.kind {
        PatchKind::Jump(target) => {
            // Look up the target block's RELATIVE offset in
            // the bytecode and add `base_offset` to make it
            // absolute. Write the absolute offset into the
            // JMP's operand — the VM reads jump operands as
            // absolute offsets in the program bytecode.
            let offset = base_offset + block_offsets[target.index()] as u32;
            bc[patch.pos] = Byte::new(Instruction::JMP).with_operand_u32(offset);
        }
        PatchKind::Branch { false_bb } => {
            // The JMPF jumps to `false_bb` when the condition
            // is false. The `true_bb` is reached by fall-through
            // to the next block in declaration order.
            let offset = base_offset + block_offsets[false_bb.index()] as u32;
            bc[patch.pos] = Byte::new(Instruction::JMPF).with_operand_u32(offset);
        }
        PatchKind::SwitchCase { tag, target } => {
            // The JUMP_IF_MATCH's tag (operands[31:16]) is
            // preserved; we patch value[31:0] with the target
            // block's absolute offset. operands[15:0] is
            // reserved (0).
            //
            // Phase 18C widened the target to a full 32-bit
            // value field, so this can address any target in
            // the bytecode.
            let offset = base_offset + block_offsets[target.index()] as u32;
            bc[patch.pos] = Byte::new(Instruction::JumpIfMatch)
                .with_operand_u32((tag & 0xFFFF) << 16)
                .with_value_u32(offset);
        }
    }
}

/// Record of a single terminator placeholder's position and the
/// target it should be patched with. See
/// [`emit_terminator_placeholder`] for emission; see
/// [`patch_terminator`] for patching.
#[derive(Debug, Clone, Copy)]
struct TerminatorPatch {
    /// What kind of terminator placeholder this is.
    kind: PatchKind,
    /// Absolute byte position of the placeholder's first byte
    /// in the bytecode Vec.
    pos: usize,
}

/// Discriminant for [`TerminatorPatch`]: which terminator
/// variant this placeholder belongs to, plus the target(s)
/// needed for patching.
#[derive(Debug, Clone, Copy)]
enum PatchKind {
    /// `Terminator::Jump(target)` — patches the JMP's operand
    /// with `block_offsets[target.index()]`.
    Jump(crate::cfg::BlockId),
    /// `Terminator::Branch { false_bb, .. }` — patches the JMPF's
    /// operand with `block_offsets[false_bb.index()]`. The
    /// `true_bb` is reached by fall-through (no patching
    /// needed).
    Branch { false_bb: crate::cfg::BlockId },
    /// `Terminator::Switch` per-case placeholder — patches the
    /// JUMP_IF_MATCH's `value[31:0]` with
    /// `block_offsets[target.index()]`. The tag is preserved in
    /// `operands[31:16]`. Each `Switch` case gets its own
    /// `SwitchCase` patch entry (the Switch terminator emits N
    /// case placeholders + 1 UNPACK).
    SwitchCase {
        tag: u32,
        target: crate::cfg::BlockId,
    },
}

/// Map a CFG [`BinOpKind`] to the corresponding stack-based
/// [`Instruction`].
///
/// **Known gaps in the existing VM** (no opcode exists for):
/// - `EqF` / `NeqF` — float equality / inequality
///
/// These panics honestly rather than silently producing wrong
/// bytecode. The existing AST/codegen never produces these
/// variants, so the linearizer is also not expected to see them
/// in Phase 0.4. Adding them is a Phase 3+ VM task.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn map_binop(op: BinOpKind) -> Byte {
    use BinOpKind::*;
    let i = match op {
        // Integer arithmetic.
        Add => Instruction::ADD,
        Sub => Instruction::SUB,
        Mul => Instruction::MUL,
        Div => Instruction::DIV,
        Mod => Instruction::MOD,
        // Float arithmetic.
        AddF => Instruction::ADDF,
        SubF => Instruction::SUBF,
        MulF => Instruction::MULF,
        DivF => Instruction::DIVF,
        ModF => Instruction::MODF,
        // Integer comparison.
        Eq => Instruction::EQ,
        Neq => Instruction::NEQ,
        Lt => Instruction::LE,
        Le => Instruction::LEQ,
        Gt => Instruction::GT,
        Ge => Instruction::GEQ,
        // Float comparison — partial. The VM lacks `EQF` and
        // `NEQF` opcodes; only the relational forms exist.
        LtF => Instruction::LEF,
        LeF => Instruction::LEQF,
        GtF => Instruction::GTF,
        GeF => Instruction::GEQF,
        // Logical.
        And => Instruction::AND,
        Or => Instruction::OR,
        // Bitwise.
        Shl => Instruction::SHL,
        Shr => Instruction::SHR,
        Xor => Instruction::XOR,
        // VM gap: float equality / inequality. The existing VM
        // doesn't have `EQF` / `NEQF` opcodes (see
        // `common/src/opcode.rs`). Panic honestly.
        EqF | NeqF => panic!(
            "Phase 0.3a linearizer: float `{}` has no VM opcode target \
             (the existing VM lacks EQF/NEQF). This is a known VM gap; \
             adding the opcode is Phase 3+.",
            op
        ),
    };
    Byte::new(i)
}

/// Map a CFG [`UnaryOpKind`] to the corresponding stack-based
/// [`Instruction`].
///
/// **Known gap in the existing VM**: there's no `NEGF` opcode —
/// only integer `NEG`. The existing AST/codegen doesn't produce
/// `NegF`, so the linearizer also doesn't expect to see it in
/// Phase 0.4. Adding the opcode is a Phase 3+ VM task.
///
/// `#[allow(dead_code)]` — see `linearize` for the rationale.
#[allow(dead_code)]
fn map_unaryop(op: UnaryOpKind) -> Byte {
    use UnaryOpKind::*;
    let i = match op {
        Neg => Instruction::NEG,
        Not => Instruction::NOT,
        NegF => panic!(
            "Phase 0.3a linearizer: float negation `-f` has no VM \
             opcode target (the existing VM lacks NEGF). This is a \
             known VM gap; adding the opcode is Phase 3+."
        ),
    };
    Byte::new(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Block, BlockId, Function, TypeRef, ValueId};

    /// Build a single-block function with no instructions and a
    /// `Return(None)` terminator. Used as the basis for the
    /// straight-line tests.
    fn fn_returning_unit(name: &str) -> Function {
        let block = Block::new(BlockId(0)).with_terminator(Terminator::Return(None));
        Function {
            name: name.to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        }
    }

    /// Build a single-block function with a single integer constant
    /// and a `Return` terminator. The const goes to `dst`, the
    /// return points at `dst`.
    fn fn_returning_const(name: &str, value: i64) -> Function {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Const { dst, value });
        block.terminator = Terminator::Return(Some(dst));
        Function {
            name: name.to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        }
    }

    // ============================================================
    // Top-level linearize
    // ============================================================

    #[test]
    fn linearize_empty_block_emits_only_return() {
        // Phase 1.6: a void return (`Return(None)`) now emits
        // `CONST 0` + `RETURN` — matching the single-pass
        // codegen's behavior (see `compiler/src/lib.rs::Expression::Function`).
        // The CONST 0 is the implicit "void" value that the VM's
        // RETURN handler pops.
        let f = fn_returning_unit("empty");
        let bc = linearize(&f, 0);
        assert_eq!(
            bc.len(),
            2,
            "expected two bytes (CONST 0 + RETURN), got {:?}",
            bc
        );
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 0);
        assert!(matches!(bc[1].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_multi_block_with_jump_succeeds_and_patches_target() {
        // Phase 1.1: multi-block CFGs no longer panic. The
        // Jump terminator's placeholder is patched with the
        // target block's offset in the second pass.
        let dst = ValueId(0);
        let mut b0 = Block::new(BlockId(0));
        b0.insts.push(Inst::Const { dst, value: 1 });
        b0.terminator = Terminator::Jump(BlockId(1));
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "multi".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // Block 0 emits: CONST (1 byte) + JMP (1 byte) = 2 bytes
        // Block 1 emits: RETURN (1 byte) = 1 byte
        // Total: 3 bytes.
        assert_eq!(bc.len(), 3);
        // Block 0 layout.
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 1);
        assert!(matches!(bc[1].bytecode(), Instruction::JMP));
        // The JMP's operand is the offset of block 1 = 2.
        assert_eq!(
            bc[1].operand_u32(),
            2,
            "JMP should target the offset of the second block"
        );
        // Block 1 layout.
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
    }

    // ============================================================
    // Constants
    // ============================================================

    #[test]
    fn linearize_const_int_emits_const_with_value() {
        let f = fn_returning_const("one", 42);
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 42, "value field carries the int");
        assert!(matches!(bc[1].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_const_negative_int_emits_const_with_value() {
        let f = fn_returning_const("neg", -7);
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // -7 as u32 (two's complement) is 0xFFFFFFF9.
        assert_eq!(bc[0].value_u32(), -7_i32 as u32);
    }

    #[test]
    fn linearize_const_bool_true_emits_const_one() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstBool { dst, value: true });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "t".to_string(),
            params: vec![],
            return_ty: TypeRef::Bool,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 1);
    }

    #[test]
    fn linearize_const_bool_false_emits_const_zero() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstBool { dst, value: false });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Bool,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert_eq!(bc[0].value_u32(), 0);
    }

    #[test]
    fn linearize_const_float_emits_const_with_bits() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstF {
            dst,
            value: 1.5_f64,
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "half".to_string(),
            params: vec![],
            return_ty: TypeRef::Float,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // The value field carries the f64 bits.
        assert_eq!(bc[0].value_u32(), 1.5_f64.to_bits() as u32);
    }

    #[test]
    fn linearize_const_string_emits_string_then_data() {
        // Phase 1.6: the bytecode order is STRING first, then
        // DATA chars. The VM's `Instruction::STRING` handler
        // reads the FOLLOWING bytes via `code.next()` and
        // pushes a string onto the stack. So the layout is:
        //
        //   [0] STRING 3
        //   [1] DATA 'a'
        //   [2] DATA 'b'
        //   [3] DATA 'c'
        //   [4] RETURN
        //
        // Pre-Phase-1.6, the linearizer emitted DATA first
        // and STRING last — which passed the byte-shape check
        // but BROKE at runtime (STRING would read the
        // following instructions as DATA chars).
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstString {
            dst,
            value: "abc".to_string(),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "s".to_string(),
            params: vec![],
            return_ty: TypeRef::String,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // 1 STRING + 3 DATA + 1 RETURN = 5.
        assert_eq!(bc.len(), 5);
        // First byte is STRING.
        assert!(matches!(bc[0].bytecode(), Instruction::STRING));
        assert_eq!(bc[0].operand_u32(), 3, "STRING operand is char count");
        // Next three are DATA.
        for (i, byte) in bc.iter().skip(1).take(3).enumerate() {
            assert!(
                matches!(byte.bytecode(), Instruction::DATA),
                "byte {} should be DATA, got {:?}",
                i + 1,
                byte.bytecode()
            );
        }
        // Last byte is RETURN.
        assert!(matches!(bc[4].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_print_emits_print_after_const_string() {
        // Phase 1.6: `Inst::Print` emits a single `PRINT` opcode
        // (no operands). The cfg_builder is expected to push
        // `Inst::ConstString` first (which emits STRING +
        // DATA chars in that order, matching the existing
        // single-pass codegen) so the string is on the stack
        // when PRINT runs.
        //
        // Bytecode layout:
        //   [0] STRING 2
        //   [1] DATA 'h'
        //   [2] DATA 'i'
        //   [3] PRINT
        //   [4] CONST 0  (implicit void return — Phase 1.6 fix)
        //   [5] RETURN
        let fmt = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::ConstString {
            dst: fmt,
            value: "hi".to_string(),
        });
        block.insts.push(Inst::Print { args: vec![fmt] });
        block.terminator = Terminator::Return(None);
        let f = Function {
            name: "p".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 6, "expected 6 bytes, got {:?}", bc);
        // STRING first.
        assert!(matches!(bc[0].bytecode(), Instruction::STRING));
        assert_eq!(bc[0].operand_u32(), 2);
        // DATA chars follow STRING.
        assert!(matches!(bc[1].bytecode(), Instruction::DATA));
        assert!(matches!(bc[2].bytecode(), Instruction::DATA));
        // PRINT.
        assert!(matches!(bc[3].bytecode(), Instruction::PRINT));
        // Void return: CONST 0 + RETURN.
        assert!(matches!(bc[4].bytecode(), Instruction::CONST));
        assert_eq!(bc[4].value_u32(), 0);
        assert!(matches!(bc[5].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_print_with_format_arg_emits_format_then_print() {
        // Phase 1.6: `print "%i", x;` — the linearizer's
        // `Inst::Print` arm must emit `FORMAT(1)` (pops 2
        // values: the format string and the one param) followed
        // by `PRINT` (pops the formatted string and prints it).
        //
        // The cfg_builder pushes the format string FIRST (via
        // `build_expression(fmt)`) and the params AFTER (via
        // `build_expression` for each param in source order),
        // so the stack at the time of `Inst::Print` is (top to
        // bottom): `param_0, format_string`. This matches the
        // single-pass codegen's stack layout (format at the
        // bottom, params stacked on top).
        //
        // Bytecode layout for `print "%i", 42;` (after the
        // cfg_builder builds the format string and the param):
        //   [0] CONST 42       (push 42 — the param)
        //   [1] STRING 2       (push "%i" — the format string)
        //   [2] DATA '%'
        //   [3] DATA 'i'
        //   [4] FORMAT 1       (pop 2 values, format them)
        //   [5] PRINT          (print the formatted string)
        //   [6] CONST 0        (implicit void return)
        //   [7] RETURN
        let fmt = ValueId(0);
        let arg = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Const {
            dst: arg,
            value: 42,
        });
        block.insts.push(Inst::ConstString {
            dst: fmt,
            value: "%i".to_string(),
        });
        block.insts.push(Inst::Print {
            args: vec![fmt, arg],
        });
        block.terminator = Terminator::Return(None);
        let f = Function {
            name: "p".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 8, "expected 8 bytes, got {:?}", bc);
        // CONST 42.
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // STRING 2.
        assert!(matches!(bc[1].bytecode(), Instruction::STRING));
        assert_eq!(bc[1].operand_u32(), 2);
        // DATA chars.
        assert!(matches!(bc[2].bytecode(), Instruction::DATA));
        assert!(matches!(bc[3].bytecode(), Instruction::DATA));
        // FORMAT 1 — pops 2 values, formats them.
        assert!(matches!(bc[4].bytecode(), Instruction::FORMAT));
        assert_eq!(
            bc[4].operand_u32(),
            1,
            "FORMAT operand is params_count (1 for one arg)"
        );
        // PRINT.
        assert!(matches!(bc[5].bytecode(), Instruction::PRINT));
        // Void return: CONST 0 + RETURN.
        assert!(matches!(bc[6].bytecode(), Instruction::CONST));
        assert_eq!(bc[6].value_u32(), 0);
        assert!(matches!(bc[7].bytecode(), Instruction::RETURN));
    }

    // ============================================================
    // Param
    // ============================================================

    #[test]
    fn linearize_param_emits_load_with_slot_index() {
        let dst = ValueId(0);
        let param_vid = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Param { dst, index: 2 });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![(param_vid, "x".to_string())],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::LOAD));
        assert_eq!(bc[0].operand_u32(), 2, "LOAD operand is the slot index");
    }

    // ============================================================
    // BinOps (sample — full mapping is exhaustively tested below)
    // ============================================================

    #[test]
    fn linearize_binop_add_emits_add() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::Add,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "add".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::ADD));
    }

    #[test]
    fn linearize_binop_all_supported_variants_map_correctly() {
        // Every BinOpKind with a VM opcode target. `EqF` and
        // `NeqF` are excluded — the VM has no EQF/NEQF opcodes;
        // see `linearize_binop_eqf_panics` below.
        let cases: &[(BinOpKind, Instruction)] = &[
            (BinOpKind::Add, Instruction::ADD),
            (BinOpKind::Sub, Instruction::SUB),
            (BinOpKind::Mul, Instruction::MUL),
            (BinOpKind::Div, Instruction::DIV),
            (BinOpKind::Mod, Instruction::MOD),
            (BinOpKind::AddF, Instruction::ADDF),
            (BinOpKind::SubF, Instruction::SUBF),
            (BinOpKind::MulF, Instruction::MULF),
            (BinOpKind::DivF, Instruction::DIVF),
            (BinOpKind::ModF, Instruction::MODF),
            (BinOpKind::Eq, Instruction::EQ),
            (BinOpKind::Neq, Instruction::NEQ),
            (BinOpKind::Lt, Instruction::LE),
            (BinOpKind::Le, Instruction::LEQ),
            (BinOpKind::Gt, Instruction::GT),
            (BinOpKind::Ge, Instruction::GEQ),
            (BinOpKind::LtF, Instruction::LEF),
            (BinOpKind::LeF, Instruction::LEQF),
            (BinOpKind::GtF, Instruction::GTF),
            (BinOpKind::GeF, Instruction::GEQF),
            (BinOpKind::And, Instruction::AND),
            (BinOpKind::Or, Instruction::OR),
            (BinOpKind::Shl, Instruction::SHL),
            (BinOpKind::Shr, Instruction::SHR),
            (BinOpKind::Xor, Instruction::XOR),
        ];
        assert_eq!(cases.len(), 25, "every supported BinOpKind must be mapped");

        for (op, expected) in cases {
            let dst = ValueId(2);
            let mut block = Block::new(BlockId(0));
            block.insts.push(Inst::BinOp {
                op: *op,
                dst,
                lhs: ValueId(0),
                rhs: ValueId(1),
            });
            block.terminator = Terminator::Return(Some(dst));
            let f = Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: TypeRef::Int,
                blocks: vec![block],
                entry: BlockId(0),
            };
            let bc = linearize(&f, 0);
            assert_eq!(bc.len(), 2);
            assert_eq!(
                bc[0].bytecode(),
                &expected.clone(),
                "BinOpKind::{:?} should emit {:?}",
                op,
                expected
            );
        }
    }

    #[test]
    #[should_panic(expected = "float `==f` has no VM opcode target")]
    fn linearize_binop_eqf_panics() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::EqF,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f, 0);
    }

    #[test]
    #[should_panic(expected = "float `!=f` has no VM opcode target")]
    fn linearize_binop_neqf_panics() {
        let dst = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::BinOp {
            op: BinOpKind::NeqF,
            dst,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f, 0);
    }

    // ============================================================
    // UnaryOps
    // ============================================================

    #[test]
    fn linearize_unaryop_supported_variants_map_correctly() {
        // `NegF` is excluded — the VM has no NEGF opcode;
        // see `linearize_unaryop_negf_panics` below.
        let cases: &[(UnaryOpKind, Instruction)] = &[
            (UnaryOpKind::Neg, Instruction::NEG),
            (UnaryOpKind::Not, Instruction::NOT),
        ];
        assert_eq!(cases.len(), 2, "supported UnaryOpKind variants");

        for (op, expected) in cases {
            let dst = ValueId(1);
            let mut block = Block::new(BlockId(0));
            block.insts.push(Inst::UnaryOp {
                op: *op,
                dst,
                src: ValueId(0),
            });
            block.terminator = Terminator::Return(Some(dst));
            let f = Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: TypeRef::Int,
                blocks: vec![block],
                entry: BlockId(0),
            };
            let bc = linearize(&f, 0);
            assert_eq!(bc.len(), 2);
            assert_eq!(
                bc[0].bytecode(),
                &expected.clone(),
                "UnaryOpKind::{:?} should emit {:?}",
                op,
                expected
            );
        }
    }

    #[test]
    #[should_panic(expected = "float negation `-f` has no VM opcode target")]
    fn linearize_unaryop_negf_panics() {
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::UnaryOp {
            op: UnaryOpKind::NegF,
            dst,
            src: ValueId(0),
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let _ = linearize(&f, 0);
    }

    // ============================================================
    // Call
    // ============================================================

    #[test]
    fn linearize_call_emits_call_then_jmp_placeholder() {
        let callee = ValueId(0);
        let dst = ValueId(3);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Call {
            dst: Some(dst),
            callee,
            args: vec![ValueId(1), ValueId(2)],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 3, "CALL + JMP + RETURN");
        assert!(matches!(bc[0].bytecode(), Instruction::CALL));
        assert_eq!(bc[0].operand_u32(), 2, "CALL operand is the arity");
        assert!(matches!(bc[1].bytecode(), Instruction::JMP));
        assert_eq!(bc[1].operand_u32(), u32::MAX, "JMP target is a placeholder");
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_call_with_no_args_emits_call_with_zero_arity() {
        let callee = ValueId(0);
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Call {
            dst: Some(dst),
            callee,
            args: vec![],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc[0].operand_u32(), 0, "CALL operand is the arity (0)");
    }

    // ============================================================
    // LoadField
    // ============================================================

    #[test]
    fn linearize_load_field_emits_load_field_with_field_index() {
        let dst = ValueId(1);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::LoadField {
            dst,
            src: ValueId(0),
            field_index: 3,
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::LoadField));
        assert_eq!(bc[0].operand_u32(), 3);
    }

    // ============================================================
    // MakeEnum
    // ============================================================

    #[test]
    fn linearize_make_enum_packs_tag_and_arity() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::MakeEnum {
            dst,
            tag: 0x1234,
            payload: vec![ValueId(1), ValueId(2), ValueId(3)],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Named(0),
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::MakeEnum));
        // operands[31:16] = tag, operands[15:0] = arity.
        let operand = bc[0].operand_u32();
        assert_eq!(operand >> 16, 0x1234, "upper 16 bits = tag");
        assert_eq!(operand & 0xFFFF, 3, "lower 16 bits = arity");
    }

    #[test]
    fn linearize_make_enum_with_zero_arity() {
        let dst = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::MakeEnum {
            dst,
            tag: 0x0007,
            payload: vec![],
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Named(0),
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        let operand = bc[0].operand_u32();
        assert_eq!(operand >> 16, 0x0007);
        assert_eq!(operand & 0xFFFF, 0);
    }

    // ============================================================
    // Unpack
    // ============================================================

    #[test]
    fn linearize_unpack_emits_unpack() {
        let dst = ValueId(1);
        let scrutinee = ValueId(0);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Unpack {
            dst: vec![dst],
            scrutinee,
        });
        block.terminator = Terminator::Return(Some(dst));
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 2);
        assert!(matches!(bc[0].bytecode(), Instruction::Unpack));
    }

    // ============================================================
    // Terminators
    // ============================================================

    #[test]
    fn linearize_unreachable_emits_halt() {
        let mut block = Block::new(BlockId(0));
        block.terminator = Terminator::Unreachable;
        let f = Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 1);
        assert!(matches!(bc[0].bytecode(), Instruction::HALT));
    }

    #[test]
    fn linearize_jump_terminator_patches_target_in_multi_block() {
        // Phase 1.1: `Jump(target)` is now supported. The
        // linearizer emits a JMP placeholder in pass 1, then
        // patches the operand with the target block's offset
        // in pass 2.
        //
        // Build a 3-block function: b0 JMP b1, b1 JMP b2,
        // b2 RETURN. b1 is reached only via the first JMP, so
        // its jump target must be patched to b2's offset.
        let dst = ValueId(0);
        let mut b0 = Block::new(BlockId(0));
        b0.insts.push(Inst::Const { dst, value: 1 });
        b0.terminator = Terminator::Jump(BlockId(1));
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 2 });
        b1.terminator = Terminator::Jump(BlockId(2));
        let b2 = Block::new(BlockId(2)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "chain".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // Block 0: CONST + JMP = 2 bytes (offset 0, 1)
        // Block 1: CONST + JMP = 2 bytes (offset 2, 3)
        // Block 2: RETURN = 1 byte (offset 4)
        assert_eq!(bc.len(), 5);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert!(matches!(bc[1].bytecode(), Instruction::JMP));
        assert_eq!(bc[1].operand_u32(), 2, "first JMP targets b1 (offset 2)");
        assert!(matches!(bc[2].bytecode(), Instruction::CONST));
        assert_eq!(bc[2].value_u32(), 2);
        assert!(matches!(bc[3].bytecode(), Instruction::JMP));
        assert_eq!(bc[3].operand_u32(), 4, "second JMP targets b2 (offset 4)");
        assert!(matches!(bc[4].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_branch_terminator_patches_jmpf_to_false_target() {
        // Phase 1.1: `Branch { true_bb, false_bb, .. }` is now
        // supported. The linearizer emits a JMPF placeholder
        // that, when patched, jumps to `false_bb` when the
        // condition is false. The `true_bb` is reached by
        // FALL-THROUGH to the next block in declaration order.
        //
        // Build a 4-block function (canonical if/else layout):
        //   b0 (entry):  no insts, Branch → (true=b2, false=b3)
        //   b1 (join):  no insts, Return
        //   b2 (then):  CONST 1, Jump → b1
        //   b3 (else):  CONST 2, Jump → b1
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(2),
            false_bb: BlockId(3),
        });
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(None));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 1 });
        b2.terminator = Terminator::Jump(BlockId(1));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 2 });
        b3.terminator = Terminator::Jump(BlockId(1));
        let f = Function {
            name: "if_else".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![b0, b1, b2, b3],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // Bytecode layout in declaration order:
        //   [b0 @ 0]      JMPF → b3 (offset 5)
        //   [b1 @ 1]      CONST 0 (implicit void), RETURN
        //   [b2 @ 3]      CONST 1, JMP → b1 (offset 1)
        //   [b3 @ 5]      CONST 2, JMP → b1 (offset 1)
        //
        // Phase 1.6: b1 now emits 2 bytes (CONST 0 + RETURN)
        // instead of 1 (RETURN only). The JMPF's target shifts
        // by 1 byte (from offset 4 to offset 5).
        assert_eq!(bc.len(), 7, "expected 7 bytes, got {:?}", bc);
        // b0's JMPF (offset 0) → b3 (offset 5).
        assert!(matches!(bc[0].bytecode(), Instruction::JMPF));
        assert_eq!(
            bc[0].operand_u32(),
            5,
            "JMPF should jump to else block (b3) when false"
        );
        // b1's CONST 0 + RETURN (offsets 1, 2).
        assert!(matches!(bc[1].bytecode(), Instruction::CONST));
        assert_eq!(bc[1].value_u32(), 0);
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
        // b2's CONST (offset 3) and JMP (offset 4) → b1 (offset 1).
        assert!(matches!(bc[3].bytecode(), Instruction::CONST));
        assert_eq!(bc[3].value_u32(), 1);
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[4].operand_u32(),
            1,
            "JMP at end of then should target join (b1)"
        );
        // b3's CONST (offset 5) and JMP (offset 6) → b1 (offset 1).
        assert!(matches!(bc[5].bytecode(), Instruction::CONST));
        assert_eq!(bc[5].value_u32(), 2);
        assert!(matches!(bc[6].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[6].operand_u32(),
            1,
            "JMP at end of else should target join (b1)"
        );
    }

    #[test]
    fn linearize_branch_with_false_going_to_join_patches_jmpf_correctly() {
        // Canonical `if cond { body }; return j;` (no else
        // branch): the Branch's false arm points DIRECTLY at
        // the join_block. The linearizer must patch the JMPF
        // to the join_block's offset.
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(2),
            false_bb: BlockId(1), // false arm → join_block directly
        });
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(None));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 7 });
        b2.terminator = Terminator::Jump(BlockId(1));
        let f = Function {
            name: "if_no_else".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![b0, b1, b2],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // Layout (Phase 1.6 — void return now emits CONST 0 + RETURN):
        //   [b0 @ 0]      JMPF → b1 (offset 1)
        //   [b1 @ 1]      CONST 0, RETURN
        //   [b2 @ 3]      CONST 7, JMP → b1 (offset 1)
        assert_eq!(bc.len(), 5);
        assert!(matches!(bc[0].bytecode(), Instruction::JMPF));
        assert_eq!(
            bc[0].operand_u32(),
            1,
            "JMPF in no-else case targets join_block (b1)"
        );
        assert!(matches!(bc[1].bytecode(), Instruction::CONST));
        assert_eq!(bc[1].value_u32(), 0);
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
        assert!(matches!(bc[3].bytecode(), Instruction::CONST));
        assert_eq!(bc[3].value_u32(), 7);
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[4].operand_u32(),
            1,
            "JMP at end of then_block also targets join_block (b1)"
        );
    }

    // ============================================================
    // Switch terminator (Phase 1.3 — Match codegen)
    // ============================================================

    #[test]
    fn linearize_switch_with_two_cases_and_default_emits_cascade() {
        // Phase 1.3: Switch linearization emits a cascade of
        // JUMP_IF_MATCH placeholders (one per case) followed by
        // a single UNPACK for the default arm.
        //
        // Build a 5-block function (canonical 3-arm match):
        //   b0 (match):   Switch → [(10, b1), (20, b2)], default b3
        //   b1 (arm_a):   CONST 100, Jump → b4
        //   b2 (arm_b):   CONST 200, Jump → b4
        //   b3 (default): CONST 300, Jump → b4
        //   b4 (join):    Return
        //
        // Expected bytecode layout:
        //   [b0 @ 0]      JUMP_IF_MATCH tag=10, target=b1 (offset 3)
        //                 JUMP_IF_MATCH tag=20, target=b2 (offset 5)
        //                 UNPACK arity=0
        //   [b1 @ 3]      CONST 100, JMP → b4 (offset 9)
        //   [b2 @ 5]      CONST 200, JMP → b4 (offset 9)
        //   [b3 @ 7]      CONST 300, JMP → b4 (offset 9)
        //   [b4 @ 9]      RETURN
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(10, BlockId(1)), (20, BlockId(2))],
            default: BlockId(3),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 100 });
        b1.terminator = Terminator::Jump(BlockId(4));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 200 });
        b2.terminator = Terminator::Jump(BlockId(4));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 300 });
        b3.terminator = Terminator::Jump(BlockId(4));
        let b4 = Block::new(BlockId(4)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "match_fn".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2, b3, b4],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // b0 emits 3 bytes; b1, b2, b3 each emit 2; b4 emits 1.
        assert_eq!(bc.len(), 10, "expected 10 bytes, got {:?}", bc);

        // b0 @ offset 0: JUMP_IF_MATCH tag=10, target=b1 (offset 3).
        assert!(matches!(bc[0].bytecode(), Instruction::JumpIfMatch));
        assert_eq!(bc[0].operand_u16(0), 10, "upper 16 bits = tag");
        assert_eq!(bc[0].operand_u16(1), 0, "lower 16 bits = reserved");
        assert_eq!(bc[0].value_u32(), 3, "value[31:0] = target offset");

        // b0 @ offset 1: JUMP_IF_MATCH tag=20, target=b2 (offset 5).
        assert!(matches!(bc[1].bytecode(), Instruction::JumpIfMatch));
        assert_eq!(bc[1].operand_u16(0), 20);
        assert_eq!(bc[1].value_u32(), 5, "value[31:0] = target offset");

        // b0 @ offset 2: UNPACK for default arm.
        assert!(matches!(bc[2].bytecode(), Instruction::Unpack));

        // b1 (arm_a) @ offset 3: CONST 100.
        assert!(matches!(bc[3].bytecode(), Instruction::CONST));
        assert_eq!(bc[3].value_u32(), 100);

        // b1 @ offset 4: JMP → b4 (offset 9).
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(bc[4].operand_u32(), 9);

        // b2 (arm_b) @ offset 5: CONST 200.
        assert!(matches!(bc[5].bytecode(), Instruction::CONST));
        assert_eq!(bc[5].value_u32(), 200);

        // b2 @ offset 6: JMP → b4 (offset 9).
        assert!(matches!(bc[6].bytecode(), Instruction::JMP));
        assert_eq!(bc[6].operand_u32(), 9);

        // b3 (default) @ offset 7: CONST 300.
        assert!(matches!(bc[7].bytecode(), Instruction::CONST));
        assert_eq!(bc[7].value_u32(), 300);

        // b3 @ offset 8: JMP → b4 (offset 9).
        assert!(matches!(bc[8].bytecode(), Instruction::JMP));
        assert_eq!(bc[8].operand_u32(), 9);

        // b4 (join) @ offset 9: RETURN.
        assert!(matches!(bc[9].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_switch_with_single_case_emits_one_jump_if_match_then_unpack() {
        // Degenerate case: 1 case + default. The Switch emits
        // exactly one JUMP_IF_MATCH followed by an UNPACK. The
        // JUMP_IF_MATCH's tag is encoded in operands[31:16] and
        // the target offset is patched into value[31:0].
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(42, BlockId(1))],
            default: BlockId(2),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 1 });
        b1.terminator = Terminator::Jump(BlockId(3));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 2 });
        b2.terminator = Terminator::Jump(BlockId(3));
        let b3 = Block::new(BlockId(3)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "single_case".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2, b3],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // b0: JUMP_IF_MATCH (1) + UNPACK (1) = 2 bytes
        // b1: CONST (1) + JMP (1) = 2 bytes
        // b2: CONST (1) + JMP (1) = 2 bytes
        // b3: RETURN (1) = 1 byte
        // Total: 7 bytes
        assert_eq!(bc.len(), 7);
        assert!(matches!(bc[0].bytecode(), Instruction::JumpIfMatch));
        assert_eq!(bc[0].operand_u16(0), 42, "tag in upper 16 bits");
        assert_eq!(
            bc[0].value_u32(),
            2,
            "target offset points to arm_a (block 1 starts at offset 2)"
        );
        assert!(matches!(bc[1].bytecode(), Instruction::Unpack));
        // b1 (arm_a) @ offset 2
        assert!(matches!(bc[2].bytecode(), Instruction::CONST));
        assert_eq!(bc[2].value_u32(), 1);
        // b2 (default) @ offset 4 — UNPACK falls through to here
        assert!(matches!(bc[4].bytecode(), Instruction::CONST));
        assert_eq!(bc[4].value_u32(), 2);
    }

    #[test]
    fn linearize_switch_with_zero_cases_emits_only_unpack() {
        // Degenerate case: 0 cases + default (e.g. `match x {
        // _ => 0 }`). The Switch emits ONLY an UNPACK (no
        // JUMP_IF_MATCH placeholders) and falls through to the
        // default arm block.
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![],
            default: BlockId(1),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 99 });
        b1.terminator = Terminator::Jump(BlockId(2));
        let b2 = Block::new(BlockId(2)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "only_default".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // b0: UNPACK (1) = 1 byte
        // b1: CONST (1) + JMP (1) = 2 bytes
        // b2: RETURN (1) = 1 byte
        // Total: 4 bytes
        assert_eq!(bc.len(), 4);
        // No JUMP_IF_MATCH anywhere.
        let has_jump_if_match = bc
            .iter()
            .any(|b| matches!(b.bytecode(), Instruction::JumpIfMatch));
        assert!(
            !has_jump_if_match,
            "zero-case Switch should not emit JUMP_IF_MATCH"
        );
        assert!(matches!(bc[0].bytecode(), Instruction::Unpack));
        assert!(matches!(bc[1].bytecode(), Instruction::CONST));
        assert_eq!(bc[1].value_u32(), 99);
        assert!(matches!(bc[2].bytecode(), Instruction::JMP));
        assert!(matches!(bc[3].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_switch_jump_if_match_target_is_wide_value_field() {
        // Regression guard: Phase 18C widened the JUMP_IF_MATCH
        // target from 16-bit to 32-bit (lives in `value[31:0]`,
        // not in `operands`). A buggy linearizer that used
        // `with_operand_u32` for the target would silently
        // truncate wide targets.
        //
        // We can't easily reach a 65,535-byte target in a unit
        // test (it would require a giant function), so we verify
        // the byte's `value_u32()` accessor returns the patched
        // target and `operand_u16(0)` (the tag's slot) is
        // independent of the target.
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(0xABCD, BlockId(1))],
            default: BlockId(2),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 0 });
        b1.terminator = Terminator::Jump(BlockId(3));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 0 });
        b2.terminator = Terminator::Jump(BlockId(3));
        let b3 = Block::new(BlockId(3)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "wide_target".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2, b3],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        let jim = &bc[0];
        // Tag in operands[31:16].
        assert_eq!(
            jim.operand_u16(0),
            0xABCD,
            "tag must be in upper 16 bits of operands"
        );
        // Reserved (write 0) in operands[15:0].
        assert_eq!(jim.operand_u16(1), 0);
        // Target in value[31:0] — independent of the tag.
        assert_eq!(
            jim.value_u32(),
            2,
            "target offset must be in value[31:0] (not in operands)"
        );
    }

    #[test]
    fn linearize_switch_does_not_emit_jump_after_unpack() {
        // The default arm is reached by FALL-THROUGH from the
        // UNPACK, not by a separate JMP. If the linearizer
        // emitted a stray JMP after the UNPACK, the default
        // block would be unreachable. The bytecode length and
        // instruction layout guard against this regression.
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(1, BlockId(1)), (2, BlockId(2))],
            default: BlockId(3),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 10 });
        b1.terminator = Terminator::Jump(BlockId(4));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 20 });
        b2.terminator = Terminator::Jump(BlockId(4));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 30 });
        b3.terminator = Terminator::Jump(BlockId(4));
        let b4 = Block::new(BlockId(4)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "no_jump_after_unpack".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2, b3, b4],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // The Switch emits: JUMP_IF_MATCH (1) + JUMP_IF_MATCH (1) + UNPACK (1) = 3 bytes
        // The default block follows immediately at offset 3.
        assert_eq!(bc.len(), 10);
        assert!(matches!(bc[2].bytecode(), Instruction::Unpack));
        // The next byte after UNPACK must be the default block's
        // first instruction, NOT a JMP.
        assert!(
            matches!(bc[3].bytecode(), Instruction::CONST),
            "UNPACK must fall through to default block (CONST expected at offset 3)"
        );
    }

    // ============================================================
    // Integration: multi-instruction sequence
    // ============================================================

    #[test]
    fn linearize_sequence_emits_instructions_in_order() {
        // Return(Const(5) + Const(3)). Both consts, then ADD, then
        // RETURN. Order matters.
        let v0 = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let mut block = Block::new(BlockId(0));
        block.insts.push(Inst::Const { dst: v0, value: 5 });
        block.insts.push(Inst::Const { dst: v1, value: 3 });
        block.insts.push(Inst::BinOp {
            op: BinOpKind::Add,
            dst: v2,
            lhs: v0,
            rhs: v1,
        });
        block.terminator = Terminator::Return(Some(v2));
        let f = Function {
            name: "add".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![block],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        assert_eq!(bc.len(), 4);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 5);
        assert!(matches!(bc[1].bytecode(), Instruction::CONST));
        assert_eq!(bc[1].value_u32(), 3);
        assert!(matches!(bc[2].bytecode(), Instruction::ADD));
        assert!(matches!(bc[3].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_empty_function_emits_no_bytecode() {
        // Defensive edge case: a function with zero blocks
        // (shouldn't occur for well-formed CFGs, but the
        // linearizer must not panic on it).
        let f = Function {
            name: "empty".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![],
            entry: BlockId::INVALID,
        };
        let bc = linearize(&f, 0);
        assert!(bc.is_empty(), "empty function should emit 0 bytes");
    }

    // ============================================================
    // base_offset (absolute jump targets)
    // ============================================================

    #[test]
    fn linearize_with_zero_base_offset_emits_relative_targets() {
        // Regression: linearize(&f, 0) must produce the SAME
        // operand values as the pre-base-offset linearize(&f).
        // (The pre-base-offset linearizer implicitly used
        // base_offset=0, so behavior for base_offset=0 must be
        // unchanged.)
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(2),
            false_bb: BlockId(3),
        });
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(None));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 1 });
        b2.terminator = Terminator::Jump(BlockId(1));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 2 });
        b3.terminator = Terminator::Jump(BlockId(1));
        let f = Function {
            name: "if_else".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![b0, b1, b2, b3],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 0);
        // Layout (Phase 1.6 — void return now emits CONST 0 + RETURN,
        // shifts every subsequent offset by +1):
        //   [b0 @ 0]      JMPF → b3 (offset 5)
        //   [b1 @ 1]      CONST 0, RETURN
        //   [b2 @ 3]      CONST 1, JMP → b1 (offset 1)
        //   [b3 @ 5]      CONST 2, JMP → b1 (offset 1)
        assert!(matches!(bc[0].bytecode(), Instruction::JMPF));
        assert_eq!(bc[0].operand_u32(), 5);
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(bc[4].operand_u32(), 1);
        assert!(matches!(bc[6].bytecode(), Instruction::JMP));
        assert_eq!(bc[6].operand_u32(), 1);
    }

    #[test]
    fn linearize_branch_with_nonzero_base_offset_patches_absolute_target() {
        // Build a 4-block if/else function (canonical layout) and
        // linearize it with base_offset = 100. The JMPF and JMP
        // operands must be 100 + (relative offset), i.e. 104, 101,
        // and 101 — not the bare relative offsets (4, 1, 1).
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Branch {
            cond: ValueId(0),
            true_bb: BlockId(2),
            false_bb: BlockId(3),
        });
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(None));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 1 });
        b2.terminator = Terminator::Jump(BlockId(1));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 2 });
        b3.terminator = Terminator::Jump(BlockId(1));
        let f = Function {
            name: "if_else_offset".to_string(),
            params: vec![],
            return_ty: TypeRef::Unit,
            blocks: vec![b0, b1, b2, b3],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 100);
        // Block offsets (Phase 1.6 — void return adds +1 byte):
        //   [b0 @ 0]      JMPF → b3 (relative offset 5)
        //   [b1 @ 1]      CONST 0, RETURN
        //   [b2 @ 3]      CONST 1, JMP → b1 (relative offset 1)
        //   [b3 @ 5]      CONST 2, JMP → b1 (relative offset 1)
        // With base_offset=100, the operands are 105, 101, 101.
        assert!(matches!(bc[0].bytecode(), Instruction::JMPF));
        assert_eq!(
            bc[0].operand_u32(),
            105,
            "JMPF must target absolute offset (100 + 5)"
        );
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[4].operand_u32(),
            101,
            "then-block JMP must target absolute offset (100 + 1)"
        );
        assert!(matches!(bc[6].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[6].operand_u32(),
            101,
            "else-block JMP must target absolute offset (100 + 1)"
        );
    }

    #[test]
    fn linearize_jump_with_nonzero_base_offset_patches_absolute_target() {
        // Build a 2-block function (b0 JMP b1, b1 RETURN) and
        // linearize with base_offset = 50. The JMP's operand must
        // be 50 + 2 = 52, not the bare relative offset of 2.
        let dst = ValueId(0);
        let mut b0 = Block::new(BlockId(0));
        b0.insts.push(Inst::Const { dst, value: 1 });
        b0.terminator = Terminator::Jump(BlockId(1));
        let b1 = Block::new(BlockId(1)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "jump_offset".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 50);
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        assert_eq!(bc[0].value_u32(), 1);
        assert!(matches!(bc[1].bytecode(), Instruction::JMP));
        assert_eq!(
            bc[1].operand_u32(),
            52,
            "JMP must target absolute offset (50 + 2)"
        );
        assert!(matches!(bc[2].bytecode(), Instruction::RETURN));
    }

    #[test]
    fn linearize_switch_with_nonzero_base_offset_patches_absolute_target() {
        // Build a 5-block 3-arm match function (see
        // linearize_switch_with_two_cases_and_default_emits_cascade)
        // and linearize with base_offset = 1000. Each JUMP_IF_MATCH's
        // value[31:0] must be 1000 + (relative target offset), and
        // each trailing JMP must be 1000 + (join offset).
        let dst = ValueId(0);
        let b0 = Block::new(BlockId(0)).with_terminator(Terminator::Switch {
            scrutinee: ValueId(0),
            cases: vec![(10, BlockId(1)), (20, BlockId(2))],
            default: BlockId(3),
        });
        let mut b1 = Block::new(BlockId(1));
        b1.insts.push(Inst::Const { dst, value: 100 });
        b1.terminator = Terminator::Jump(BlockId(4));
        let mut b2 = Block::new(BlockId(2));
        b2.insts.push(Inst::Const { dst, value: 200 });
        b2.terminator = Terminator::Jump(BlockId(4));
        let mut b3 = Block::new(BlockId(3));
        b3.insts.push(Inst::Const { dst, value: 300 });
        b3.terminator = Terminator::Jump(BlockId(4));
        let b4 = Block::new(BlockId(4)).with_terminator(Terminator::Return(Some(dst)));
        let f = Function {
            name: "match_offset".to_string(),
            params: vec![],
            return_ty: TypeRef::Int,
            blocks: vec![b0, b1, b2, b3, b4],
            entry: BlockId(0),
        };
        let bc = linearize(&f, 1000);
        // Layout (same as
        // linearize_switch_with_two_cases_and_default_emits_cascade):
        //   [b0 @ 0]      JUMP_IF_MATCH tag=10 → b1 (rel=3) → abs=1003
        //                 JUMP_IF_MATCH tag=20 → b2 (rel=5) → abs=1005
        //                 UNPACK arity=0
        //   [b1 @ 3]      CONST 100, JMP → b4 (rel=9) → abs=1009
        //   [b2 @ 5]      CONST 200, JMP → b4 (rel=9) → abs=1009
        //   [b3 @ 7]      CONST 300, JMP → b4 (rel=9) → abs=1009
        //   [b4 @ 9]      RETURN
        assert!(matches!(bc[0].bytecode(), Instruction::JumpIfMatch));
        assert_eq!(bc[0].operand_u16(0), 10);
        assert_eq!(
            bc[0].value_u32(),
            1003,
            "JUMP_IF_MATCH must target absolute offset (1000 + 3)"
        );
        assert!(matches!(bc[1].bytecode(), Instruction::JumpIfMatch));
        assert_eq!(bc[1].operand_u16(0), 20);
        assert_eq!(
            bc[1].value_u32(),
            1005,
            "JUMP_IF_MATCH must target absolute offset (1000 + 5)"
        );
        // Trailing JMPs in arm blocks all target the join block
        // (rel offset 9 → abs 1009).
        assert!(matches!(bc[4].bytecode(), Instruction::JMP));
        assert_eq!(bc[4].operand_u32(), 1009);
        assert!(matches!(bc[6].bytecode(), Instruction::JMP));
        assert_eq!(bc[6].operand_u32(), 1009);
        assert!(matches!(bc[8].bytecode(), Instruction::JMP));
        assert_eq!(bc[8].operand_u32(), 1009);
    }
}
