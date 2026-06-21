//! Deferred-target bytecode emission for control flow.
//!
//! `BlockBuilder` is a basic-block builder for the codegen pass. It
//! owns a local `Vec<Byte>` and a set of "labels" — opaque
//! placeholders for forward-jump targets. Jumps are emitted with
//! a placeholder operand (zero); the placeholder is patched when
//! the target label is bound.
//!
//! # Absolute offsets (Phase 16.5)
//!
//! Every `BlockBuilder` is constructed with a `base: u32` — the
//! absolute position in the parent bytecode buffer where these
//! bytes will eventually be appended. All jump targets produced
//! by the builder are stored as **absolute** offsets:
//! `base + relative_position_in_bytes`.
//!
//! This means once the builder's bytes are appended to the
//! parent buffer, the jump targets are correct *without a
//! relocation pass*. The pre-16.5 design used 0-based local
//! offsets and required a separate `relocate_jumps(bytecode,
//! base)` post-pass to convert them to absolute — that pass was
//! removed in Phase 16.5.
//!
//! # Composition
//!
//! Each `do_compile` call that needs control flow creates its OWN
//! `BlockBuilder` with `base = self.bytecode.len() as u32` and
//! appends the finalized bytes back to `self.bytecode`.
//!
//! Children that themselves contain jumps are compiled into their
//! own `BlockBuilder` with a new `base = current base + offset`
//! (where `offset` is the position of the child's bytes in the
//! parent's local buffer). Because each builder is constructed
//! with its own base, nested control flow "just works" — there is
//! no mixed-coordinate-systems hazard.
//!
//! # Example
//!
//! ```ignore
//! let mut bb = BlockBuilder::new(self.bytecode.len() as u32);
//!
//! // Emit <cond> then JMPF to "end".
//! bb.extend(self.do_compile(cond));
//! let end = bb.emit_jump(JumpKind::JumpIfFalse);
//!
//! // Emit <then-body>.
//! bb.extend(self.do_compile(then_body));
//!
//! // Bind "end" to the current position.
//! bb.bind_label(end);
//!
//! let result = bb.finalize().expect("all labels bound");
//! bytecode.extend(result);
//! ```

use std::collections::{BTreeMap, BTreeSet};

use common::{Byte, Instruction};

#[cfg(test)]
use common::Value;

/// Opaque handle for a forward-jump target. Two `BlockBuilder`s
/// must never share labels unless one is a child of the other
/// (label IDs are globally unique within a single `compile()`
/// invocation).
///
/// We use a separate `Label` type (not `common::Label`, which is
/// for ariadne diagnostics) to avoid confusion in the codegen.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label(usize);

impl Label {
    /// The numeric ID of this label. Used by
    /// [`BlockBuilder::fresh_label`] internally and exposed for
    /// debugging / assertions.
    #[allow(dead_code)] // Test-only accessor.
    pub fn id(self) -> usize {
        self.0
    }
}

/// What kind of jump to emit. Carries enough info to construct
/// the placeholder. The `target` operand is filled in at
/// `bind_label` time.
///
/// `JumpIfMatch::arity` is accepted for API symmetry with the
/// instruction's full layout, but the operand only stores the
/// `tag` (upper 16 bits) and the target offset (lower 16 bits).
/// The VM reads the real arity from the enum at runtime — see
/// the comment in `common/src/opcode.rs` for the operand layout.
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

/// Result of [`BlockBuilder::finalize`]. Failure means at least
/// one allocated label was never bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// `label` was allocated via [`BlockBuilder::fresh_label`]
    /// but never bound via [`BlockBuilder::bind_label`].
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

/// A basic-block builder with deferred-target jumps that
/// produces **absolute** offsets.
///
/// # Absolute offsets
///
/// `base` is the absolute position in the parent bytecode
/// buffer where these bytes will eventually be appended. All
/// jump targets produced by `emit_jump` / `emit_jump_to` are
/// `base + relative_position_in_bytes` once their label is
/// bound. Because the targets are absolute from the moment
/// they are patched, appending the finalized bytes to the
/// parent buffer produces correct bytecode without a
/// relocation pass.
///
/// # Invariant
///
/// Every [`BlockBuilder::emit_jump`] and
/// [`BlockBuilder::emit_jump_to`] call allocates or targets a
/// label. The label must eventually be bound via
/// [`BlockBuilder::bind_label`] (or updated via
/// [`BlockBuilder::rebind_label`] if the binding moves), or
/// `finalize` returns [`BlockError::UnboundLabel`].
///
/// # Non-idempotency (AMENDMENT 3)
///
/// `bind_label` is **non-idempotent**: calling it twice on the
/// same label PANICS with a clear message. For the rare update
/// case (loop back-edge semantics where the binding moves
/// after the body has been emitted), use [`rebind_label`].
///
/// [`rebind_label`]: BlockBuilder::rebind_label
pub struct BlockBuilder {
    bytes: Vec<Byte>,
    /// Absolute position in the parent buffer where these bytes
    /// will be appended. ALL jump targets produced by this
    /// builder are `base + relative_position`.
    base: u32,
    next_label: usize,
    /// For each label id, the list of operand positions in `bytes`
    /// (one per emitted jump targeting that label) that should
    /// be patched. Entries are KEPT (not removed) on `bind_label`
    /// so that `rebind_label` can re-patch them with a new
    /// position.
    pending: BTreeMap<usize, Vec<usize>>,
    /// Set of label ids that have been bound. Used to enforce
    /// AMENDMENT 3 (non-idempotency of `bind_label`) and to
    /// check `rebind_label`'s precondition.
    bound: BTreeSet<usize>,
}

impl Default for BlockBuilder {
    /// Default to `base = 0` — useful in tests and for
    /// standalone use, but production codegen always passes an
    /// explicit base equal to `self.bytecode.len()`.
    fn default() -> Self {
        Self::new(0)
    }
}

impl BlockBuilder {
    /// Create an empty builder anchored at absolute offset
    /// `base`. All jump targets produced by this builder are
    /// `base + relative_position`.
    ///
    /// In production codegen, `base` is always
    /// `self.bytecode.len() as u32` at the moment the builder
    /// is created.
    pub fn new(base: u32) -> Self {
        Self {
            bytes: Vec::new(),
            base,
            next_label: 0,
            pending: BTreeMap::new(),
            bound: BTreeSet::new(),
        }
    }

    /// The absolute base offset this builder was constructed
    /// with. Exposed for debugging / assertions; not used by
    /// current production codegen.
    #[allow(dead_code)] // Accessor for tests.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// Allocate a new label id. The label is NOT bound yet.
    /// Returns a label that no other `BlockBuilder` shares.
    pub fn fresh_label(&mut self) -> Label {
        let id = self.next_label;
        self.next_label += 1;
        Label(id)
    }

    /// Bind `label` to the current absolute position
    /// (`base + self.bytes.len()`).
    ///
    /// Patches every previously-emitted jump that targeted
    /// `label` with this position. The label is marked as bound;
    /// calling `bind_label` again on the same label PANICS
    /// (AMENDMENT 3).
    pub fn bind_label(&mut self, label: Label) {
        if !self.bound.insert(label.0) {
            panic!(
                "BlockBuilder::bind_label called twice on label {:?} \
                 (AMENDMENT 3: bind_label is non-idempotent; \
                 use rebind_label for the update case)",
                label
            );
        }
        let position = self.base + self.bytes.len() as u32;
        self.patch_pending(label, position);
    }

    /// Update the binding of `label` to `new_position` (an
    /// ABSOLUTE offset). Re-patches every previously-emitted
    /// jump that targeted `label` with `new_position`. Used by
    /// loops to update the top-of-loop binding after the body
    /// has been emitted.
    ///
    /// Panics if `label` was never bound.
    #[allow(dead_code)] // Reserved for future complex control flow.
    pub fn rebind_label(&mut self, label: Label, new_position: u32) {
        if !self.bound.contains(&label.0) {
            panic!(
                "BlockBuilder::rebind_label called on unbound label {:?}",
                label
            );
        }
        self.patch_pending(label, new_position);
    }

    /// Append a single byte.
    #[allow(dead_code)] // Single-byte emit — current production codegen uses `extend`.
    pub fn emit(&mut self, byte: Byte) {
        self.bytes.push(byte);
    }

    /// Append a sequence of bytes. Equivalent to
    /// `self.bytes.extend_from_slice(&bytes)`.
    pub fn extend(&mut self, bytes: Vec<Byte>) {
        self.bytes.extend(bytes);
    }

    /// Emit a jump placeholder with a FRESHLY ALLOCATED label
    /// as the target. The caller must later `bind_label` the
    /// returned label. Equivalent to
    /// `let l = self.fresh_label(); self.emit_jump_to(l, kind); l`.
    pub fn emit_jump(&mut self, kind: JumpKind) -> Label {
        let label = self.fresh_label();
        self.emit_jump_to(label, kind);
        label
    }

    /// Emit a jump placeholder targeting an EXISTING label
    /// (e.g., a backward jump to the top of a loop, or a
    /// forward jump to `end_label` of an `if` chain).
    pub fn emit_jump_to(&mut self, target: Label, kind: JumpKind) {
        let byte_pos = self.bytes.len();
        let byte = match kind {
            JumpKind::Unconditional => {
                Byte::new(Instruction::JMP).with_operand_u32(0)
            }
            JumpKind::JumpIfFalse => {
                Byte::new(Instruction::JMPF).with_operand_u32(0)
            }
            JumpKind::JumpIfTrue => {
                Byte::new(Instruction::JMPT).with_operand_u32(0)
            }
            JumpKind::JumpIfMatch { tag, .. } => {
                // Upper 16 bits = tag, lower 16 bits = target
                // (placeholder = 0; patched on bind_label).
                // `arity` is intentionally discarded — the VM
                // reads the real arity from the enum at runtime.
                Byte::new(Instruction::JumpIfMatch)
                    .with_operands_u16([tag as u16, 0])
            }
        };
        self.bytes.push(byte);
        // Record that this byte should be patched when
        // `target` is bound. We keep the entry (don't remove
        // on bind) so `rebind_label` can re-patch.
        self.pending.entry(target.0).or_default().push(byte_pos);
    }

    /// Current **absolute** position in the parent buffer
    /// (`base + self.bytes.len()`). Returns the position at
    /// which the next emitted byte would land once the
    /// finalized bytes are appended to the parent.
    #[allow(dead_code)] // Test-only — production codegen binds labels via `bind_label`.
    pub fn current_position(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }

    /// Finalize: validate that every allocated label is bound,
    /// then return the local byte buffer. All jump targets in
    /// the returned buffer are **absolute** (`base +
    /// relative_position`), ready to be appended to the
    /// parent without any post-pass.
    pub fn finalize(self) -> Result<Vec<Byte>, BlockError> {
        for label_id in 0..self.next_label {
            if !self.bound.contains(&label_id) {
                return Err(BlockError::UnboundLabel(Label(label_id)));
            }
        }
        Ok(self.bytes)
    }

    /// Internal helper: patch every pending jump that targets
    /// `label` with `position` (an absolute offset).
    fn patch_pending(&mut self, label: Label, position: u32) {
        if let Some(operand_positions) = self.pending.get(&label.0) {
            for pos in operand_positions {
                Self::patch_jump_operand(&mut self.bytes, *pos, position);
            }
        }
    }

    /// Patch the operand of a jump at `byte_pos` to point to
    /// `target` (an absolute offset). Preserves the tag (upper
    /// 16 bits) for `JumpIfMatch`; replaces the entire operand
    /// for `JMP`/`JMPF`/`JMPT`.
    fn patch_jump_operand(bytes: &mut [Byte], byte_pos: usize, target: u32) {
        let byte = &mut bytes[byte_pos];
        match byte.bytecode() {
            Instruction::JMP | Instruction::JMPF | Instruction::JMPT => {
                *byte = byte.with_operand_u32(target);
            }
            Instruction::JumpIfMatch => {
                // Preserve the tag in the upper 16 bits.
                let tag = (byte.operand_u32() >> 16) as u16;
                // The target is a 16-bit bytecode offset
                // (matching the existing `JumpIfMatch` operand
                // layout documented in `common/src/opcode.rs`).
                // Panicking on overflow is the right behaviour:
                // the 15C design explicitly documents the
                // 65,535-byte ceiling.
                let target_u16 = u16::try_from(target).unwrap_or_else(|_| {
                    panic!(
                        "JumpIfMatch target offset {} overflows u16 \
                         (Phase 15D.5 MEDIUM #1: the JUMP_IF_MATCH \
                         operand layout caps targets at 65,535 bytes)",
                        target
                    )
                });
                *byte = byte.with_operands_u16([tag, target_u16]);
            }
            other => panic!(
                "BlockBuilder: patch_jump_operand called on non-jump \
                 instruction {:?} at position {}",
                other, byte_pos
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut bb = BlockBuilder::new(0);
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

    // ---- 2. bind_label_patches_pending_jump (base 0) --------------

    /// With `base = 0`, after `emit_jump(Jmp); emit(byte);
    /// bind_label`, the JMP target should equal `1` (base + 1
    /// byte emitted = absolute position 1 in the parent
    /// buffer). This is the simplest absolute-target case.
    #[test]
    fn bind_label_patches_pending_jump() {
        let mut bb = BlockBuilder::new(0);
        let target = bb.fresh_label();

        // Emit a JMP placeholder targeting `target`.
        bb.emit_jump_to(target, JumpKind::Unconditional);
        // Push 3 padding bytes (CONSTs).
        bb.emit(const_int(1));
        bb.emit(const_int(2));
        bb.emit(const_int(3));
        // `target` should bind to absolute position 4
        // (base 0 + 4 bytes emitted).
        bb.bind_label(target);

        let result = bb.finalize().expect("all labels bound");
        // The JMP is at byte 0. Its operand should be 4.
        assert_eq!(result.len(), 4);
        assert!(matches!(result[0].bytecode(), Instruction::JMP));
        assert_eq!(result[0].operand_u32(), 4);
    }

    // ---- 2b. bind_label_uses_absolute_base (base > 0) --------------

    /// With `base = 100`, after `emit_jump(Jmp); emit(byte);
    /// bind_label`, the JMP target should equal `101`
    /// (base 100 + 1 byte emitted). This is the load-bearing
    /// Phase 16.5 test: confirms targets are absolute, not
    /// 0-based relative.
    #[test]
    fn bind_label_uses_absolute_base() {
        let mut bb = BlockBuilder::new(100);
        let target = bb.fresh_label();

        bb.emit_jump_to(target, JumpKind::Unconditional);
        bb.emit(const_int(7));
        bb.emit(const_int(8));
        bb.emit(const_int(9));
        bb.bind_label(target);

        let result = bb.finalize().expect("all labels bound");
        assert_eq!(result.len(), 4);
        assert!(matches!(result[0].bytecode(), Instruction::JMP));
        // 100 (base) + 4 (bytes emitted) = 104
        assert_eq!(result[0].operand_u32(), 104);
    }

    // ---- 3. bind_label_twice_panics (AMENDMENT 3) -----------------

    /// AMENDMENT 3: `bind_label` is non-idempotent. Calling
    /// it twice on the same label MUST panic. This is the
    /// load-bearing safety check — a non-panicking
    /// re-binding would silently corrupt jump targets.
    #[test]
    fn bind_label_twice_panics() {
        let mut bb = BlockBuilder::new(0);
        let l = bb.fresh_label();
        bb.bind_label(l);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bb.bind_label(l);
        }));
        assert!(
            result.is_err(),
            "bind_label twice on the same label should panic (AMENDMENT 3)"
        );
    }

    // ---- 4. rebind_label_updates_jump_target (AMENDMENT 3) -------

    /// AMENDMENT 3 update case: a label is bound, then
    /// re-bound via `rebind_label` to a NEW ABSOLUTE position.
    /// All pending jumps targeting the label should be
    /// re-patched to the new absolute position.
    #[test]
    fn rebind_label_updates_jump_target() {
        let mut bb = BlockBuilder::new(0);
        let target = bb.fresh_label();

        // Emit a JMP placeholder.
        bb.emit_jump_to(target, JumpKind::Unconditional);
        bb.emit(const_int(1));
        // First bind — JMP should target byte 2 (base 0 + 1 JMP + 1 CONST).
        bb.bind_label(target);

        // Emit another jump to the SAME label.
        bb.emit_jump_to(target, JumpKind::JumpIfFalse);
        bb.emit(const_int(2));
        // Current absolute position = 4. Rebind to absolute 4
        // — both jumps now target absolute byte 4.
        bb.rebind_label(target, 4);

        let result = bb.finalize().expect("all labels bound");
        assert_eq!(result.len(), 4);
        assert!(matches!(result[0].bytecode(), Instruction::JMP));
        assert_eq!(result[0].operand_u32(), 4);
        assert!(matches!(result[2].bytecode(), Instruction::JMPF));
        assert_eq!(result[2].operand_u32(), 4);
    }

    /// `rebind_label` on an unbound label MUST panic.
    #[test]
    fn rebind_label_on_unbound_label_panics() {
        let mut bb = BlockBuilder::new(0);
        let l = bb.fresh_label();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bb.rebind_label(l, 42);
        }));
        assert!(
            result.is_err(),
            "rebind_label on an unbound label should panic"
        );
    }

    // ---- 5. emit_jmp_uses_correct_opcode --------------------------

    /// Each `JumpKind` variant emits the corresponding
    /// `Instruction` opcode. The operand is 0 (placeholder) at
    /// emit time. With `base = 1`, the bound target is 2 (one
    /// JMP byte emitted).
    #[test]
    fn emit_jmp_uses_correct_opcode() {
        // JMP
        let mut bb = BlockBuilder::new(1);
        let l = bb.fresh_label();
        bb.emit_jump_to(l, JumpKind::Unconditional);
        bb.bind_label(l);
        let r = bb.finalize().unwrap();
        assert!(matches!(r[0].bytecode(), Instruction::JMP));
        assert_eq!(r[0].operand_u32(), 2);

        // JMPF
        let mut bb = BlockBuilder::new(1);
        let l = bb.fresh_label();
        bb.emit_jump_to(l, JumpKind::JumpIfFalse);
        bb.bind_label(l);
        let r = bb.finalize().unwrap();
        assert!(matches!(r[0].bytecode(), Instruction::JMPF));
        assert_eq!(r[0].operand_u32(), 2);

        // JMPT
        let mut bb = BlockBuilder::new(1);
        let l = bb.fresh_label();
        bb.emit_jump_to(l, JumpKind::JumpIfTrue);
        bb.bind_label(l);
        let r = bb.finalize().unwrap();
        assert!(matches!(r[0].bytecode(), Instruction::JMPT));
        assert_eq!(r[0].operand_u32(), 2);
    }

    // ---- 6. emit_jump_if_match_preserves_tag ----------------------

    /// For `JumpIfMatch`, the tag (upper 16 bits) is set at
    /// emit time. The target (lower 16 bits) is patched on
    /// bind to the absolute offset. After bind, the tag should
    /// be preserved.
    #[test]
    fn emit_jump_if_match_preserves_tag() {
        let mut bb = BlockBuilder::new(0);
        let target = bb.fresh_label();
        // Tag = 0x1234, arity = 99 (arity is discarded).
        bb.emit_jump_to(target, JumpKind::JumpIfMatch { tag: 0x1234, arity: 99 });
        bb.emit(const_int(1));
        // `target` absolute position = base 0 + 2 bytes = 2.
        bb.bind_label(target);

        let r = bb.finalize().unwrap();
        assert!(matches!(r[0].bytecode(), Instruction::JumpIfMatch));
        let operand = r[0].operand_u32();
        let tag = (operand >> 16) as u16;
        let target = (operand & 0xFFFF) as u16;
        assert_eq!(tag, 0x1234, "tag should be preserved");
        assert_eq!(target, 2, "target should be patched to absolute position 2");
    }

    // ---- 7. emit_jump_to_uses_existing_label ---------------------

    /// Multiple jumps to the same label are all patched to
    /// the same absolute target.
    #[test]
    fn emit_jump_to_uses_existing_label() {
        let mut bb = BlockBuilder::new(0);
        let target = bb.fresh_label();
        // Three jumps to the same label.
        bb.emit_jump_to(target, JumpKind::Unconditional);
        bb.emit(const_int(1));
        bb.emit_jump_to(target, JumpKind::Unconditional);
        bb.emit(const_int(2));
        bb.emit_jump_to(target, JumpKind::Unconditional);
        bb.emit(const_int(3));
        // Bind to absolute end = base 0 + 6 bytes = 6.
        bb.bind_label(target);

        let r = bb.finalize().unwrap();
        // All three JMPs (at bytes 0, 2, 4) should target absolute byte 6.
        for (i, b) in r.iter().enumerate() {
            if matches!(b.bytecode(), Instruction::JMP) {
                assert_eq!(b.operand_u32(), 6, "JMP at byte {} should target 6", i);
            }
        }
    }

    // ---- 8. finalize_returns_unbound_label_error -----------------

    /// If any label is allocated but never bound,
    /// `finalize` returns `Err(BlockError::UnboundLabel(_))`.
    #[test]
    fn finalize_returns_unbound_label_error() {
        let mut bb = BlockBuilder::new(0);
        // Allocate two labels; bind only one.
        let bound_label = bb.fresh_label();
        let unbound_label = bb.fresh_label();

        bb.emit_jump_to(bound_label, JumpKind::Unconditional);
        bb.emit_jump_to(unbound_label, JumpKind::Unconditional);
        bb.bind_label(bound_label);
        // DON'T bind `unbound_label`.

        let result = bb.finalize();
        assert!(result.is_err(), "finalize should fail with unbound label");
        match result {
            Err(BlockError::UnboundLabel(l)) => {
                assert_eq!(l, unbound_label);
            }
            Ok(_) => panic!("expected error"),
        }
    }

    // ---- 9. finalize_returns_bytecode_in_order -------------------

    /// `finalize` succeeds when every allocated label is
    /// bound.
    #[test]
    fn finalize_returns_bytecode_in_order() {
        let mut bb = BlockBuilder::new(0);
        let end = bb.fresh_label();

        bb.emit(const_int(1));
        bb.emit_jump_to(end, JumpKind::Unconditional);
        bb.emit(const_int(2));
        bb.bind_label(end);
        bb.emit(const_int(3));

        let r = bb.finalize().expect("all labels bound");
        // Order: CONST 1, JMP 3, CONST 2, CONST 3.
        assert_eq!(r.len(), 4);
        assert!(matches!(r[0].bytecode(), Instruction::CONST));
        assert!(matches!(r[1].bytecode(), Instruction::JMP));
        assert_eq!(r[1].operand_u32(), 3);
        assert!(matches!(r[2].bytecode(), Instruction::CONST));
        assert!(matches!(r[3].bytecode(), Instruction::CONST));
    }

    // ---- 10. extend_appends_in_order ------------------------------

    /// `extend` appends the bytes in order. We use a fresh
    /// `BlockBuilder` per branch and then `extend` the
    /// child's bytes into the parent — this is the
    /// composition pattern.
    #[test]
    fn extend_appends_in_order() {
        let mut bb = BlockBuilder::new(0);
        // First child: CONST 1, CONST 2.
        bb.extend(vec![const_int(1), const_int(2)]);
        // Second child: CONST 3.
        bb.extend(vec![const_int(3)]);
        // Third child: empty.
        bb.extend(vec![]);

        let r = bb.finalize().expect("empty finalize ok");
        assert_eq!(r.len(), 3);
        for (i, want) in [1, 2, 3].iter().enumerate() {
            assert!(matches!(r[i].bytecode(), Instruction::CONST));
            assert_eq!(r[i].constant(), *want as u64);
        }
    }

    // ---- 11. current_position_is_absolute -------------------------

    /// `current_position` returns the absolute position
    /// (`base + self.bytes.len()`), not just the local
    /// buffer length. Phase 16.5 production codegen relies
    /// on this when binding labels to specific positions.
    #[test]
    fn current_position_is_absolute() {
        let mut bb = BlockBuilder::new(500);
        assert_eq!(bb.current_position(), 500, "empty builder at base 500");
        bb.emit(const_int(1));
        assert_eq!(bb.current_position(), 501);
        bb.emit(const_int(2));
        assert_eq!(bb.current_position(), 502);
    }

    // ---- 12. base_accessor_returns_constructor_value --------------

    /// `base` returns the constructor argument.
    #[test]
    fn base_accessor_returns_constructor_value() {
        let bb = BlockBuilder::new(1234);
        assert_eq!(bb.base(), 1234);
        let bb = BlockBuilder::new(0);
        assert_eq!(bb.base(), 0);
    }

    // ---- 13. finalize_targets_ready_without_relocation -----------

    /// Phase 16.5's whole reason for existing: targets in
    /// the finalized buffer are correct WITHOUT a
    /// post-pass. We simulate "appending to a parent
    /// buffer" by pre-pending non-zero base to all
    /// expected targets, and verify the targets land at
    /// the right positions.
    #[test]
    fn finalize_targets_ready_without_relocation() {
        // Simulate appending to a parent buffer at base 1000.
        let mut bb = BlockBuilder::new(1000);
        let end = bb.fresh_label();

        bb.emit(const_int(1));
        bb.emit_jump_to(end, JumpKind::JumpIfFalse);
        bb.emit(const_int(2));
        bb.bind_label(end);
        bb.emit(const_int(3));

        let r = bb.finalize().expect("all labels bound");
        // JMPF should target absolute position 1003 (1000 + 3),
        // the position right after the `CONST 2` byte. This is
        // where the `end` label binds to.
        assert!(matches!(r[1].bytecode(), Instruction::JMPF));
        assert_eq!(r[1].operand_u32(), 1003);
    }

    // ---- 14. jump_if_match_target_uses_absolute_with_base --------

    /// `JumpIfMatch` target is patched to the absolute
    /// position. With `base = 50` and 3 bytes emitted, the
    /// target lands at 53.
    #[test]
    fn jump_if_match_target_uses_absolute_with_base() {
        let mut bb = BlockBuilder::new(50);
        let target = bb.fresh_label();
        bb.emit_jump_to(target, JumpKind::JumpIfMatch { tag: 0x0042, arity: 1 });
        bb.emit(const_int(1));
        bb.emit(const_int(2));
        bb.bind_label(target);

        let r = bb.finalize().unwrap();
        assert!(matches!(r[0].bytecode(), Instruction::JumpIfMatch));
        let operand = r[0].operand_u32();
        let tag = (operand >> 16) as u16;
        let tgt = (operand & 0xFFFF) as u16;
        assert_eq!(tag, 0x0042);
        assert_eq!(tgt, 53, "JumpIfMatch target = base 50 + 3 bytes");
    }
}