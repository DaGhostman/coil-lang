use std::{
    borrow::{Borrow, BorrowMut},
    fmt::{Debug, Display},
};

#[derive(Default, PartialEq, Debug, Copy, Clone)]
pub enum Operation {
    #[default]
    Noop,
    Halt,
    // ---
    Add,
    BitAnd,
    BitOr,
    BitXor,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Pow,
    LeftShift,
    RightShift,
    Or,
    Xor,
    And,
    // ---
    Jump,
    Rewind,
    ConditionJump,
    // ---
    /// Begin an independent scope that is a copy of the previous one, reusing the IP
    Begin,
    /// Leave independent scope without resetting the IP
    End,
    Enter,
    Leave,
    Apply,
    Call,
    Invoke,
    // ---
    Pop,
    Push,
    Load,
    Store,
    Duplicate,
    // ---
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    // ---
    Print,
    Match,
    Check,
    Range,
    // ---
    Array,
    // ---
    Unwrap,
    UnwrapError,

    // Experimental
    Condition,
}

#[derive(PartialEq, Debug, Copy, Clone)]
pub struct Metadata {
    line: usize,
    column: usize,
    // file: String,
}

#[derive(Copy, Clone, PartialEq)]
pub struct IR {
    code: Operation,
    operands: [usize; 3],
    metadata: Option<Metadata>,
}

impl Debug for IR {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({})",
            self.code,
            self.operands
                .to_vec()
                .iter()
                .filter(|v| !matches!(v, 0))
                .collect::<Vec<&usize>>()
                .len(),
        )
    }
}

impl IR {
    pub fn new(code: Operation, values: Option<[usize; 3]>) -> Self {
        Self {
            code,
            operands: values.unwrap_or_default(),
            metadata: None,
        }
    }

    pub fn operands(&self) -> &[usize] {
        self.operands.borrow()
    }

    pub fn operands_mut(&mut self) -> &mut [usize] {
        self.operands.borrow_mut()
    }

    pub fn attach_metadata(&mut self, metadata: Metadata) {
        self.metadata = Some(metadata);
    }

    pub fn code(&self) -> Operation {
        self.code
    }

    pub fn get(&self, idx: usize) -> Option<&usize> {
        self.operands.get(idx)
    }

    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Byte {
    /// Terminate the execution of the program (unrecoverable)
    Halt,
    /// Suspend the execution of the program (recoverable)
    Pause,
    /// Spawn a new instance of the machine
    Spawn,
    /// Join a spawned thread
    Join,
    /// Enter a stack frame
    Enter,
    /// Duplicate current frame and enter a new scope
    Scope,
    /// Leave a stack frame
    Leave,
    /// Jump to relative offset
    Jump,
    /// Conditionally jump to relative offset
    Jumpz,
    /// Sum float values
    AddFloat,
    /// Sum integer values
    AddInteger,
    /// Concatenate string
    ConcatString,
    /// Cast value to integer
    IntCast,
    /// Cast value to float
    FloatCast,
    /// Cast value to string
    StringCast,
    /// Cast value to boolean
    BoolCast,
    /// Push a value on the stack
    Push,
    /// Pop a value from the stack
    Pop,
    /// Load a variable
    Load,
    /// Store a variable
    Store,
    /// Create an array
    Array,

    /// Prints the value from the top of the stack
    Print,
    /// A range of numbers
    Range,

    /// Call a function
    Call,

    /// Equality Check
    Equal,
}

#[derive(Clone, Debug)]
pub struct Code {
    byte: Byte,
    operands: Option<Vec<usize>>,
}

impl Code {
    pub fn new(byte: Byte) -> Self {
        Self {
            byte,
            operands: None,
        }
    }

    pub fn with_operands(&mut self, operands: Vec<usize>) {
        self.operands = Some(operands);
    }

    pub fn byte(&self) -> Byte {
        self.byte
    }

    pub fn operand(&self, idx: usize) -> Option<usize> {
        if let Some(operand) = self.operands.clone() {
            operand.get(idx).copied()
        } else {
            None
        }
    }

    pub fn bits(&self) -> Vec<u8> {
        let mut bytes = (self.byte() as u8).to_le_bytes().to_vec();
        if let Some(operands) = &self.operands {
            bytes.push(operands.len() as u8);
            for op in operands {
                bytes.append(&mut op.to_le_bytes().to_vec());
            }
        } else {
            bytes.push(0u8);
        }

        bytes
    }
}
