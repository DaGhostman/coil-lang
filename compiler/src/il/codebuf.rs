//! Bytecode buffer backed by stack IL during emit; lowered at finalize.

use std::collections::HashMap;

use common::{Byte, DebugLoc, Instruction};

use super::{
    EntryKind, IlBuilder, IlJumpKind, IlOp, Label, Lowered, lower,
};

/// Compile-time code buffer: IL during emit, `Vec<Byte>` after lower.
#[derive(Clone, Default)]
pub struct CodeBuf {
    il: IlBuilder,
    lowered: Option<Vec<Byte>>,
    lowered_locs: Option<Vec<DebugLoc>>,
    /// Logical code index → entry label from [`Self::bind_fresh_entry`].
    /// Used to rewrite packed CALL/CodePtr Bytes into [`IlOp::Entry`].
    entry_at_offset: HashMap<usize, Label>,
}

impl CodeBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn il(&self) -> &IlBuilder {
        &self.il
    }

    pub fn il_mut(&mut self) -> &mut IlBuilder {
        // Error-recovery may mutate IL after a prior lower.
        self.lowered = None;
        self.lowered_locs = None;
        &mut self.il
    }

    pub fn push(&mut self, b: Byte) {
        // Error-recovery paths may emit after a failed finalize/lower.
        self.lowered = None;
        self.lowered_locs = None;
        if let Some((kind, arity, label)) = self.entry_from_abs_byte(b) {
            self.il.emit_entry(kind, arity, label);
        } else {
            self.il.push_byte(b);
        }
    }

    pub fn extend<I: IntoIterator<Item = Byte>>(&mut self, iter: I) {
        for b in iter {
            self.push(b);
        }
    }

    /// If `b` is a call-like op targeting a known entry PC, return Entry parts.
    fn entry_from_abs_byte(&self, b: Byte) -> Option<(EntryKind, u32, Label)> {
        match *b.bytecode() {
            Instruction::CALL => {
                let (arity, target) = b.call_parts();
                let label = *self.entry_at_offset.get(&target)?;
                Some((EntryKind::Call, arity as u32, label))
            }
            Instruction::TailCall => {
                let (arity, target) = b.call_parts();
                let label = *self.entry_at_offset.get(&target)?;
                Some((EntryKind::TailCall, arity as u32, label))
            }
            Instruction::MakeCoro => {
                let (arity, target) = b.call_parts();
                let label = *self.entry_at_offset.get(&target)?;
                Some((EntryKind::MakeCoro, arity as u32, label))
            }
            Instruction::CodePtr => {
                let target = b.operand_u32() as usize;
                let label = *self.entry_at_offset.get(&target)?;
                Some((EntryKind::CodePtr, 0, label))
            }
            Instruction::MakePolyFn => {
                let target = b.operand_u32() as usize;
                let label = *self.entry_at_offset.get(&target)?;
                Some((EntryKind::MakePolyFn, 0, label))
            }
            _ => None,
        }
    }

    pub fn append(&mut self, other: &mut Vec<Byte>) {
        self.extend(other.drain(..));
    }

    pub fn len(&self) -> usize {
        match &self.lowered {
            Some(v) => v.len(),
            None => self.il.code_len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.il.clear();
        self.lowered = None;
        self.lowered_locs = None;
        self.entry_at_offset.clear();
    }

    pub fn fresh_label(&mut self) -> Label {
        self.il.fresh_label()
    }

    pub fn bind_label(&mut self, label: Label) {
        self.il.bind_label(label);
    }

    /// Bind a fresh label at the current emit position (fn / lambda / thunk entry).
    /// Labels do not advance [`Self::len`], so absolute PC tables stay aligned.
    /// Records the binding so later packed CALL/CodePtr Bytes rewrite to Entry.
    pub fn bind_fresh_entry(&mut self) -> Label {
        let pc = self.len();
        let label = self.fresh_label();
        self.bind_label(label);
        self.entry_at_offset.insert(pc, label);
        label
    }

    /// Look up the entry label bound at logical code index `pc`, if any.
    pub fn entry_label_for_offset(&self, pc: usize) -> Option<Label> {
        self.entry_at_offset.get(&pc).copied()
    }

    /// Return an existing label bound at logical code index `code_pos`, or insert
    /// and bind a fresh one. Used by static-init JMP → `main` reconciliation.
    pub fn entry_label_at(&mut self, code_pos: usize) -> Label {
        let mut emitting = 0usize;
        let mut raw_idx = self.il.raw_len();
        let mut existing: Option<Label> = None;
        for (i, op) in self.il.ops().iter().enumerate() {
            if emitting == code_pos {
                if let IlOp::Label(l) = op {
                    existing = Some(*l);
                } else {
                    raw_idx = i;
                }
                break;
            }
            if op.emits_code() {
                emitting += 1;
            }
        }
        if let Some(l) = existing {
            return l;
        }
        let label = self.il.fresh_label();
        self.il.insert_bound_label_at(raw_idx, label);
        self.entry_at_offset.entry(code_pos).or_insert(label);
        label
    }

    pub fn emit_jump(&mut self, kind: IlJumpKind, target: Label) {
        self.il.emit_jump(kind, target);
    }

    pub fn emit_entry(&mut self, kind: EntryKind, arity: u32, target: Label) {
        self.il.emit_entry(kind, arity, target);
    }

    pub fn push_prologue_jmp(&mut self) {
        self.il.push_prologue_jmp();
    }

    pub fn splice_bytes_at(&mut self, code_pos: usize, bytes: Vec<Byte>) {
        let mut inserted = IlBuilder::new();
        inserted.extend_bytes(bytes);
        self.il.splice_code_at(code_pos, inserted);
    }

    pub fn insert_jump_at(&mut self, code_pos: usize, target: Label) {
        let mut emitting = 0usize;
        let mut raw_idx = self.il.raw_len();
        for (i, op) in self.il.ops().iter().enumerate() {
            if emitting == code_pos {
                raw_idx = i;
                break;
            }
            if op.emits_code() {
                emitting += 1;
            }
        }
        self.il.ops_mut().insert(
            raw_idx,
            IlOp::Jump {
                kind: IlJumpKind::Unconditional,
                target,
                loc: DebugLoc::unknown(),
            },
        );
    }

    pub fn lower_in_place(&mut self, pool: &mut Vec<u64>) -> Lowered {
        let lowered = lower(self.il.ops(), pool);
        self.lowered = Some(lowered.bytecode.clone());
        self.lowered_locs = Some(lowered.debug_locs.clone());
        lowered
    }

    pub fn as_slice(&self) -> &[Byte] {
        self.lowered.as_deref().unwrap_or(&[])
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<Byte> {
        self.lowered.get_or_insert_with(Vec::new)
    }

    pub fn clone_bytes(&self) -> Vec<Byte> {
        self.lowered.clone().unwrap_or_default()
    }

    pub fn take_bytes(&mut self) -> Vec<Byte> {
        self.lowered.take().unwrap_or_default()
    }

    pub fn ops(&self) -> &[IlOp] {
        self.il.ops()
    }

    pub fn lowered_locs(&self) -> &[DebugLoc] {
        self.lowered_locs.as_deref().unwrap_or(&[])
    }

    pub fn set_loc_on_last(&mut self, loc: DebugLoc) {
        if let Some(op) = self.il.ops_mut().last_mut() {
            op.set_loc(loc);
        }
    }

    /// Truncate to `code_len` emitting ops. Labels bound at PC `code_len`
    /// (entry labels for the next instruction) are preserved; emitting ops
    /// at that PC and beyond are dropped.
    pub fn truncate(&mut self, code_len: usize) {
        assert!(self.lowered.is_none());
        let mut emitting = 0usize;
        let mut keep = 0usize;
        for (i, op) in self.il.ops().iter().enumerate() {
            if op.emits_code() {
                if emitting == code_len {
                    break;
                }
                emitting += 1;
                keep = i + 1;
            } else if emitting > code_len {
                break;
            } else {
                // Keep labels/markers at PCs `<= code_len` (incl. entry binds).
                keep = i + 1;
            }
        }
        self.il.ops_mut().truncate(keep);
        self.entry_at_offset.retain(|&pc, _| pc < code_len);
    }

    /// Plain bytes in the emitting-op range `[start, end)` (labels skipped).
    /// Jump/Entry ops are omitted from the returned vec — callers that need
    /// a faithful body copy must reject spans with [`Self::span_has_control_ops`].
    pub fn code_slice_bytes(&self, start: usize, end: usize) -> Vec<Byte> {
        let mut out = Vec::new();
        let mut i = 0usize;
        for op in self.il.ops() {
            if let Some(b) = op.as_plain_byte() {
                if i >= start && i < end {
                    out.push(b);
                }
                i += 1;
                if i >= end {
                    break;
                }
            } else if op.emits_code() {
                i += 1;
                if i >= end {
                    break;
                }
            }
        }
        out
    }

    /// True if `[start, end)` contains a Jump/Entry (not safe to tiny-inline
    /// via [`Self::code_slice_bytes`], which drops those ops).
    pub fn span_has_control_ops(&self, start: usize, end: usize) -> bool {
        let mut i = 0usize;
        for op in self.il.ops() {
            if !op.emits_code() {
                continue;
            }
            if i >= end {
                break;
            }
            if i >= start {
                match op {
                    super::IlOp::Jump { .. }
                    | super::IlOp::Entry { .. }
                    | super::IlOp::PrologueJmp { .. } => return true,
                    _ => {}
                }
            }
            i += 1;
        }
        false
    }

    pub fn insert_byte_at_code(&mut self, code_idx: usize, byte: Byte) {
        let mut emitting = 0usize;
        let mut raw_idx = self.il.raw_len();
        for (i, op) in self.il.ops().iter().enumerate() {
            if emitting == code_idx {
                raw_idx = i;
                break;
            }
            if op.emits_code() {
                emitting += 1;
            }
        }
        self.il.ops_mut().insert(raw_idx, IlOp::byte(byte));
    }

    /// Shift [`Self::entry_at_offset`] keys after a splice that inserts `delta`
    /// emitting ops at `threshold`. Entry ops themselves are symbolic and need
    /// no rewrite; leftover abs CALL/CodePtr Bytes (defer target 0, missing
    /// CodePtr 0) are below any real entry PC and are left untouched.
    pub fn bump_absolute_entry_targets(&mut self, threshold: usize, delta: usize) {
        if delta == 0 {
            return;
        }
        let mut next = HashMap::with_capacity(self.entry_at_offset.len());
        for (pc, label) in self.entry_at_offset.drain() {
            let pc = if pc >= threshold { pc + delta } else { pc };
            next.insert(pc, label);
        }
        self.entry_at_offset = next;
    }

    pub fn last_byte(&self) -> Option<Byte> {
        for op in self.il.ops().iter().rev() {
            if let Some(b) = op.as_plain_byte() {
                return Some(b);
            }
            if op.emits_code() {
                return None;
            }
        }
        None
    }
}
