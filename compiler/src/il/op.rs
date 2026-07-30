//! Stack IL opcodes and symbolic labels.

use common::{Byte, DebugLoc, Instruction};

/// Opaque forward/back-edge target resolved once at lower time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label(pub u32);

impl Label {
    pub fn id(self) -> u32 {
        self.0
    }
}

/// Control-flow jump kind (IL-level; packing happens in the lowerer).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IlJumpKind {
    Unconditional,
    JumpIfFalse,
    JumpIfTrue,
    JumpIfMatch { tag: u32, arity: u32 },
}

/// Call-like entry that carries a symbolic label instead of a PC.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Call,
    TailCall,
    MakeCoro,
    CodePtr,
    MakePolyFn,
}

/// One IL instruction. Labels occupy no final bytecode slot.
#[derive(Clone, PartialEq, Eq)]
pub enum IlOp {
    /// Ordinary VM instruction whose operand is not a code pointer.
    /// Jump/call ops that still embed absolute PCs are accepted for
    /// transitional emit paths; prefer [`IlOp::Jump`] / [`IlOp::Entry`].
    Byte { byte: Byte, loc: DebugLoc },
    /// Bind `label` to the next emitting instruction's PC (last bind wins).
    Label(Label),
    /// Control-flow jump with a symbolic target.
    Jump {
        kind: IlJumpKind,
        target: Label,
        loc: DebugLoc,
    },
    /// CALL / TailCall / MakeCoro / CodePtr / MakePolyFn with a label target.
    Entry {
        kind: EntryKind,
        arity: u32,
        target: Label,
        loc: DebugLoc,
    },
    /// Prologue JMP placeholder (`u32::MAX`); patched by the pipeline after lower.
    PrologueJmp { loc: DebugLoc },
}

impl IlOp {
    pub fn byte(byte: Byte) -> Self {
        Self::Byte {
            byte,
            loc: DebugLoc::unknown(),
        }
    }

    pub fn byte_at(byte: Byte, loc: DebugLoc) -> Self {
        Self::Byte { byte, loc }
    }

    /// True if this op becomes one (or more) final bytecode slots.
    pub fn emits_code(&self) -> bool {
        !matches!(self, IlOp::Label(_))
    }

    pub fn loc(&self) -> DebugLoc {
        match self {
            IlOp::Byte { loc, .. }
            | IlOp::Jump { loc, .. }
            | IlOp::Entry { loc, .. }
            | IlOp::PrologueJmp { loc } => *loc,
            IlOp::Label(_) => DebugLoc::unknown(),
        }
    }

    pub fn set_loc(&mut self, loc: DebugLoc) {
        match self {
            IlOp::Byte { loc: l, .. }
            | IlOp::Jump { loc: l, .. }
            | IlOp::Entry { loc: l, .. }
            | IlOp::PrologueJmp { loc: l } => *l = loc,
            IlOp::Label(_) => {}
        }
    }

    /// If this is a plain `Byte` jump/call that still carries an absolute PC,
    /// return that PC for transitional tooling. Prefer symbolic forms.
    pub fn as_plain_byte(&self) -> Option<Byte> {
        match self {
            IlOp::Byte { byte, .. } => Some(*byte),
            _ => None,
        }
    }

    pub fn instruction(&self) -> Option<Instruction> {
        match self {
            IlOp::Byte { byte, .. } => Some(*byte.bytecode()),
            IlOp::Jump { kind, .. } => Some(match kind {
                IlJumpKind::Unconditional => Instruction::JMP,
                IlJumpKind::JumpIfFalse => Instruction::JMPF,
                IlJumpKind::JumpIfTrue => Instruction::JMPT,
                IlJumpKind::JumpIfMatch { .. } => Instruction::JumpIfMatch,
            }),
            IlOp::Entry { kind, .. } => Some(match kind {
                EntryKind::Call => Instruction::CALL,
                EntryKind::TailCall => Instruction::TailCall,
                EntryKind::MakeCoro => Instruction::MakeCoro,
                EntryKind::CodePtr => Instruction::CodePtr,
                EntryKind::MakePolyFn => Instruction::MakePolyFn,
            }),
            IlOp::PrologueJmp { .. } => Some(Instruction::JMP),
            IlOp::Label(_) => None,
        }
    }
}
