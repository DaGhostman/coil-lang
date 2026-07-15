//! Deferred-target jump patching for control-flow codegen.
//!
//! `BlockBuilder` does not own a byte buffer. All bytes go to an external
//! `Vec<Byte>` (typically `Compiler::bytecode`); `pending` records absolute
//! placeholder positions in that buffer and `bind_label` patches them.
//! This avoids coordinate mismatches when nested emitters (e.g. `Print`)
//! write directly to `Compiler::bytecode`.
//!
//! `bind_label` is idempotent: rebinding updates every jump to that label.
//! [`BlockBuilder::finalize`] errors if a targeted label was never bound.

use std::collections::{BTreeMap, BTreeSet};

use common::{Byte, Instruction};

/// Opaque forward-jump target (distinct from ariadne's `common::Label`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label(u32);

impl Label {
    /// The numeric ID of this label. Exposed for debugging / test
    /// assertions only.
    #[allow(dead_code)] // Test-only accessor.
    pub fn id(self) -> u32 {
        self.0
    }
}

/// Jump placeholder kind. The target operand is patched at `bind_label`.
#[allow(dead_code)] // JumpIfTrue / JumpIfMatch are reserved for future Match codegen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JumpKind {
    /// `Instruction::JMP` — unconditional jump.
    Unconditional,
    /// `Instruction::JMPF` — pop; jump if the popped value is
    /// `false`.
    JumpIfFalse,
    /// `Instruction::JMPT` — pop; jump if the popped value is
    /// `true`.
    JumpIfTrue,
    /// `Instruction::JumpIfMatch` — peek; jump if the scrutinee's
    /// tag matches `tag`. The `arity` is for API symmetry; the VM
    /// reads the real arity from the enum at runtime.
    JumpIfMatch { tag: u32, arity: u32 },
}

/// Result of [`BlockBuilder::finalize`]. Failure means at least one
/// allocated label was never bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// `label` was allocated via [`BlockBuilder::fresh_label`] or
    /// [`BlockBuilder::emit_jump`] but never bound via
    /// [`BlockBuilder::bind_label`].
    UnboundLabel(Label),
}

impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockError::UnboundLabel(label) => {
                write!(f, "label {:?} was never bound", label)
            }
        }
    }
}

impl std::error::Error for BlockError {}

pub struct BlockBuilder {
    /// label id → bytecode positions of jumps targeting that label
    pending: BTreeMap<u32, Vec<usize>>,
    /// labels that have been bound at least once
    bound: BTreeSet<u32>,
    next_label_id: u32,
}

impl Default for BlockBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockBuilder {
    /// Create an empty placeholder tracker.
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            bound: BTreeSet::new(),
            next_label_id: 0,
        }
    }

    /// Allocate a new label id. The label is NOT bound yet. Returns
    /// a label that no other `BlockBuilder` shares (label IDs are
    /// globally unique within a single `BlockBuilder`).
    pub fn fresh_label(&mut self) -> Label {
        let id = self.next_label_id;
        self.next_label_id += 1;
        Label(id)
    }

    /// Emit a jump placeholder with a FRESHLY ALLOCATED label as
    /// the target. The caller must later `bind_label` the returned
    /// label to set the target. See [`BlockBuilder::emit_jump_to`]
    /// for the more flexible variant.
    /// label. Equivalent to
    /// `let l = self.fresh_label(); self.emit_jump_to(l, kind, bytecode); l`.
    #[allow(dead_code)] // Not used by the current If codegen; reserved for future
    // control-flow emitters (e.g., Loop, While).
    pub fn emit_jump(&mut self, kind: JumpKind, bytecode: &mut Vec<Byte>) -> Label {
        let label = self.fresh_label();
        self.emit_jump_to(label, kind, bytecode);
        label
    }

    /// Emit a jump placeholder targeting an EXISTING label
    /// (e.g., a forward jump to `end_label` of an `if` chain).
    /// The placeholder's operand is `0`; it will be patched by
    /// `bind_label`.
    pub fn emit_jump_to(&mut self, target: Label, kind: JumpKind, bytecode: &mut Vec<Byte>) {
        let byte_pos = bytecode.len();
        let byte = Self::make_jump_placeholder(kind);
        bytecode.push(byte);
        // Record that this byte position should be patched when
        // `target` is bound. We keep the entry (don't remove on
        // bind) so re-binding re-patches.
        self.pending.entry(target.0).or_default().push(byte_pos);
    }

    /// Bind `label` to an ABSOLUTE target position in `bytecode`.
    /// Patches every pending jump that targets `label` with
    /// `target`. **Idempotent**: calling again on the same label
    /// re-patches every pending jump with the new target.
    pub fn bind_label(
        &mut self,
        label: Label,
        target: u32,
        bytecode: &mut [Byte],
        pool: &mut Vec<u64>,
    ) {
        self.bound.insert(label.0);
        if let Some(positions) = self.pending.get(&label.0) {
            for pos in positions {
                Self::patch_jump_operand(bytecode, *pos, target, pool);
            }
        }
    }

    /// Validate that every label that was targeted by an emitted
    /// jump has been bound at least once via `bind_label`.
    /// Returns `Err(BlockError::UnboundLabel(_))` if not.
    ///
    /// A label that was allocated via `fresh_label` but never
    /// targeted by any jump is allowed (it has no effect on the
    /// bytecode; this is harmless).
    pub fn finalize(self) -> Result<(), BlockError> {
        for label_id in self.pending.keys() {
            if !self.bound.contains(label_id) {
                return Err(BlockError::UnboundLabel(Label(*label_id)));
            }
        }
        Ok(())
    }

    /// Internal helper: construct the placeholder byte for a given
    /// `JumpKind`. The placeholder's operand is `0`; the caller is
    /// responsible for patching via `bind_label`.
    fn make_jump_placeholder(kind: JumpKind) -> Byte {
        match kind {
            JumpKind::Unconditional => Byte::new(Instruction::JMP).with_operand_u32(0),
            JumpKind::JumpIfFalse => Byte::new(Instruction::JMPF).with_operand_u32(0),
            JumpKind::JumpIfTrue => Byte::new(Instruction::JMPT).with_operand_u32(0),
            JumpKind::JumpIfMatch { tag, .. } => {
                Byte::new(Instruction::JumpIfMatch).with_operands_u16([tag as u16, 0])
            }
        }
    }

    /// Patch the operand of a jump at `byte_pos` to point to
    /// `target`. For `JumpIfMatch`, appends `target` to `pool`
    /// and stores the pool index in the lower 16 bits.
    fn patch_jump_operand(
        bytecode: &mut [Byte],
        byte_pos: usize,
        target: u32,
        pool: &mut Vec<u64>,
    ) {
        let byte = &mut bytecode[byte_pos];
        match byte.bytecode() {
            Instruction::JMP | Instruction::JMPF | Instruction::JMPT => {
                *byte = byte.with_operand_u32(target);
            }
            Instruction::JumpIfMatch => {
                let tag = (byte.operand_u32() >> 16) as u16;
                let pool_idx = pool.len() as u16;
                pool.push(target as u64);
                *byte = Byte::new(Instruction::JumpIfMatch).with_operands_u16([tag, pool_idx]);
            }
            other => panic!(
                "BlockBuilder: patch_jump_operand called on non-jump \
                 instruction {} at position {}",
                *other as u8, byte_pos
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::Value;

    // ---- helpers ---------------------------------------------------

    /// Build a `CONST` byte that pushes `i64` onto the stack.
    /// Used as a "do something" stand-in for branch bodies in
    /// the tests below.
    fn const_int(value: i64) -> Byte {
        Byte::new_with_value(Instruction::CONST, Value::from(value).raw() as _)
    }

    // ---- 1. fresh_label_returns_unique_ids ------------------------

    /// `fresh_label` allocates monotonically increasing label
    /// ids. The ids are unique within a single `BlockBuilder`.
    #[test]
    fn fresh_label_returns_unique_ids() {
        let mut bb = BlockBuilder::new();
        let a = bb.fresh_label();
        let b = bb.fresh_label();
        let c = bb.fresh_label();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(a.id(), 0);
        assert_eq!(b.id(), 1);
        assert_eq!(c.id(), 2);
    }

    // ---- 2. emit_jump_appends_placeholder --------------------------

    /// `emit_jump(Jmp, &mut bc)` pushes a JMP byte with operand=0
    /// to `bc` and returns a fresh label.
    #[test]
    fn emit_jump_appends_placeholder() {
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        bc.push(const_int(1));
        bc.push(const_int(2));

        let l = bb.emit_jump(JumpKind::Unconditional, &mut bc);

        // The placeholder was appended.
        assert_eq!(bc.len(), 3);
        assert!(matches!(bc[2].bytecode(), Instruction::JMP));
        assert_eq!(bc[2].operand_u32(), 0, "placeholder operand should be 0");
        // The returned label is fresh.
        assert_eq!(l.id(), 0);
    }

    // ---- 3. emit_jump_to_records_position --------------------------

    /// `emit_jump_to(label, Jmp, &mut bc)` records the JMP's
    /// absolute position in `pending[label.id]`.
    #[test]
    fn emit_jump_to_records_position() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();

        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);

        assert_eq!(bc.len(), 3);
        // All three positions are recorded for `l`.
        assert!(bb.pending.contains_key(&l.id()));
        assert_eq!(bb.pending[&l.id()].len(), 3);
        assert_eq!(bb.pending[&l.id()], &[0, 1, 2]);
    }

    // ---- 4. bind_label_patches_jump --------------------------------

    /// After `emit_jump_to(label, Jmp, &mut bc); bind_label(label,
    /// 100, &mut bc);` the JMP's operand is `100`.
    #[test]
    fn bind_label_patches_jump() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        bc.push(const_int(1));
        bc.push(const_int(2));
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(3));
        // The JMP is at position 2; bind it to absolute position
        // 5 (past the final CONST).
        bb.bind_label(l, 5, &mut bc, &mut pool);

        assert!(matches!(bc[2].bytecode(), Instruction::JMP));
        assert_eq!(bc[2].operand_u32(), 5);
    }

    // ---- 5. bind_label_preserves_jump_if_match_tag -----------------

    /// For `JUMP_IF_MATCH { tag: 5, arity: 1 }`, after
    /// `bind_label(label, 100_000, &mut bc)`, the byte's
    /// `operands` have tag=5 in the upper 16 bits (lower 16
    /// bits reserved) and `value[31:0]` is the patched 32-bit
    /// target (> 65,535 exercises the wide-target path).
    #[test]
    fn bind_label_preserves_jump_if_match_tag() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump_to(l, JumpKind::JumpIfMatch { tag: 5, arity: 1 }, &mut bc);
        bb.bind_label(l, 100_000, &mut bc, &mut pool);

        assert!(matches!(bc[0].bytecode(), Instruction::JumpIfMatch));
        let operand = bc[0].operand_u32();
        let tag = (operand >> 16) as u16;
        assert_eq!(tag, 5, "tag should be preserved in upper 16 bits");
        assert_eq!(
            operand & 0xFFFF,
            0,
            "lower 16 bits should hold pool index 0"
        );
        let target = bc[0].jump_if_match_target(&pool);
        assert!(
            target > 0xFFFF,
            "test must exercise wide-target path (target={})",
            target
        );
        assert_eq!(target, 100_000, "target should be patched");
    }

    // ---- 6. bind_label_patches_multiple_jumps_to_same_label --------

    /// Multiple `emit_jump_to(label, ...)` calls, then
    /// `bind_label(label, 100, ...)` patches all of them.
    #[test]
    fn bind_label_patches_multiple_jumps_to_same_label() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(1));
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(2));
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);

        bb.bind_label(l, 100, &mut bc, &mut pool);

        // All three JMPs (at positions 0, 2, 4) target 100.
        for pos in [0, 2, 4] {
            assert!(
                matches!(bc[pos].bytecode(), Instruction::JMP),
                "byte at position {} should be JMP",
                pos
            );
            assert_eq!(
                bc[pos].operand_u32(),
                100,
                "JMP at position {} should target 100",
                pos
            );
        }
    }

    // ---- 7. bind_label_is_idempotent -------------------------------

    /// Calling `bind_label` twice with different targets: the
    /// second call updates the patches to the new target.
    #[test]
    fn bind_label_is_idempotent() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);

        bb.bind_label(l, 50, &mut bc, &mut pool);
        assert_eq!(bc[0].operand_u32(), 50);

        bb.bind_label(l, 200, &mut bc, &mut pool);
        assert_eq!(bc[0].operand_u32(), 200);
    }

    // ---- 8. bind_label_only_patches_target_label -------------------

    /// Multiple labels, `bind_label(label_a, 100, ...)` doesn't
    /// affect jumps targeting `label_b`.
    #[test]
    fn bind_label_only_patches_target_label() {
        let mut bb = BlockBuilder::new();
        let a = bb.fresh_label();
        let b = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump_to(a, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(1));
        bb.emit_jump_to(b, JumpKind::Unconditional, &mut bc);

        bb.bind_label(a, 100, &mut bc, &mut pool);

        // JMP at position 0 (target A) should be 100.
        assert_eq!(bc[0].operand_u32(), 100);
        // JMP at position 2 (target B) should still be 0 (not
        // patched).
        assert_eq!(bc[2].operand_u32(), 0);
    }

    // ---- 9. emit_jump_jmpf_jmpt_use_correct_opcode -----------------

    /// Each `JumpKind` variant produces the right `Instruction`.
    #[test]
    fn emit_jump_jmpf_jmpt_use_correct_opcode() {
        // JMP
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump(JumpKind::Unconditional, &mut bc);
        assert!(matches!(bc[0].bytecode(), Instruction::JMP));

        // JMPF
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump(JumpKind::JumpIfFalse, &mut bc);
        assert!(matches!(bc[0].bytecode(), Instruction::JMPF));

        // JMPT
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump(JumpKind::JumpIfTrue, &mut bc);
        assert!(matches!(bc[0].bytecode(), Instruction::JMPT));

        // JUMP_IF_MATCH
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        bb.emit_jump(
            JumpKind::JumpIfMatch {
                tag: 0xABCD,
                arity: 1,
            },
            &mut bc,
        );
        assert!(matches!(bc[0].bytecode(), Instruction::JumpIfMatch));
        assert_eq!((bc[0].operand_u32() >> 16) as u16, 0xABCD);
    }

    // ---- 10. finalize_returns_unbound_label_error ------------------

    /// Emit a jump to a label, never bind, `finalize` returns Err.
    #[test]
    fn finalize_returns_unbound_label_error() {
        let mut bb = BlockBuilder::new();
        let mut bc: Vec<Byte> = Vec::new();
        let _l = bb.emit_jump(JumpKind::Unconditional, &mut bc);
        // Don't bind `_l`.

        let result = bb.finalize();
        assert!(result.is_err(), "finalize should fail with unbound label");
        match result {
            Err(BlockError::UnboundLabel(_)) => (),
            Ok(_) => panic!("expected error"),
        }
    }

    // ---- 11. finalize_succeeds_when_all_labels_bound ---------------

    /// Emit a jump to a label, bind it, finalize succeeds.
    #[test]
    fn finalize_succeeds_when_all_labels_bound() {
        let mut bb = BlockBuilder::new();
        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        let l = bb.emit_jump(JumpKind::Unconditional, &mut bc);
        bb.bind_label(l, 100, &mut bc, &mut pool);
        assert!(bb.finalize().is_ok());
    }

    // ---- 12. integrated_test_with_bytecode --------------------------

    /// Simulate a simple if/else using BlockBuilder on an external
    /// `Vec<Byte>`, verify the JMPF and JMP targets are correct.
    ///
    /// Layout produced for `if c { b1 } else { b2 }`:
    ///   c, JMPF → end, b1, JMP end, b2, [end]
    #[test]
    fn integrated_test_with_bytecode() {
        let mut bb = BlockBuilder::new();
        let end_label = bb.fresh_label();

        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();

        // Emit <cond> (CONST 1).
        bc.push(const_int(1));
        // Emit JMPF placeholder → end_label.
        bb.emit_jump_to(end_label, JumpKind::JumpIfFalse, &mut bc);
        // Emit <then-body> (CONST 2).
        bc.push(const_int(2));
        // Emit JMP → end_label.
        bb.emit_jump_to(end_label, JumpKind::Unconditional, &mut bc);
        // Emit <else-body> (CONST 3).
        bc.push(const_int(3));

        // Bind end_label to current bytecode.len().
        let end_pos = bc.len() as u32;
        bb.bind_label(end_label, end_pos, &mut bc, &mut pool);

        // Assert: bytecode has 5 bytes total.
        assert_eq!(bc.len(), 5);
        // Byte 0: CONST 1.
        assert!(matches!(bc[0].bytecode(), Instruction::CONST));
        // Byte 1: JMPF, operand = end_pos (= 5).
        assert!(matches!(bc[1].bytecode(), Instruction::JMPF));
        assert_eq!(bc[1].operand_u32(), 5);
        // Byte 2: CONST 2.
        assert!(matches!(bc[2].bytecode(), Instruction::CONST));
        // Byte 3: JMP, operand = end_pos (= 5).
        assert!(matches!(bc[3].bytecode(), Instruction::JMP));
        assert_eq!(bc[3].operand_u32(), 5);
        // Byte 4: CONST 3.
        assert!(matches!(bc[4].bytecode(), Instruction::CONST));

        // Finalize succeeds.
        assert!(bb.finalize().is_ok());
    }

    // ---- 13. emit_jump_to_after_bind_label_records_new_pending -----

    /// Emit a jump, bind, then emit another jump targeting the
    /// SAME label. The second emit appends a new entry to
    /// `pending`. A subsequent `bind_label` re-patches BOTH
    /// Rebinding updates all jumps to the same label.
    #[test]
    fn emit_jump_to_after_bind_label_records_new_pending() {
        let mut bb = BlockBuilder::new();
        let l = bb.fresh_label();
        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();

        // First jump.
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(1));
        bb.bind_label(l, 10, &mut bc, &mut pool);
        // First jump now targets 10.
        assert_eq!(bc[0].operand_u32(), 10);

        // Second jump (after the first was bound).
        bb.emit_jump_to(l, JumpKind::Unconditional, &mut bc);
        bc.push(const_int(2));
        // The second jump has placeholder operand 0.
        assert_eq!(bc[2].operand_u32(), 0);

        // Re-bind — both jumps should now target 20.
        bb.bind_label(l, 20, &mut bc, &mut pool);
        assert_eq!(bc[0].operand_u32(), 20);
        assert_eq!(bc[2].operand_u32(), 20);
    }

    // ---- 14. fresh_label_without_jump_is_fine ----------------------

    /// A `fresh_label` call that is never used in a jump is OK
    /// at `finalize` time (it's an unused label, not an unbound
    /// one).
    #[test]
    fn fresh_label_without_jump_is_fine() {
        let mut bb = BlockBuilder::new();
        let _l = bb.fresh_label(); // never used
        let mut pool = Vec::new();
        let mut bc: Vec<Byte> = Vec::new();
        let l2 = bb.emit_jump(JumpKind::Unconditional, &mut bc);
        bb.bind_label(l2, 100, &mut bc, &mut pool);
        assert!(bb.finalize().is_ok());
    }
}
