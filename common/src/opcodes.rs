use std::{
    borrow::{Borrow, BorrowMut},
    fmt::{Debug, Display},
};

#[derive(Default, PartialEq, Debug, Copy, Clone)]
#[repr(u8)]
pub enum Operation {
    #[default]
    Noop,
    Halt,
    // ---
    Add,
    Negate,
    Not,
    BitAnd,
    BitOr,
    BitXor,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Pow,
    Inc,
    Dec,
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
    Iterate,
    // ---
    Pop,
    Const,
    Declare,
    Load,
    Upvalue,
    Store,
    Assign,
    Duplicate,
    Argument,
    Function,
    Closure,
    Interface,
    Implement,
    Class,
    Prop,
    Method,
    Instantiate,
    Bind,
    This,
    TypeOf,
    PropAssign,
    PropLoad,
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
    Yield,
    // ---
    Unwrap,
    UnwrapError,

    // Experimental
    Condition,
    Loop,
}

#[derive(PartialEq, Default, Debug, Clone, Copy)]
pub struct Metadata {
    line: usize,
    column: usize,
}

impl Metadata {
    #[must_use]
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

impl Display for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Copy, Clone, PartialEq)]
pub struct IR {
    code: Operation,
    operands: [usize; 3],
    metadata: Metadata,
    r#type: usize,
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
    #[must_use]
    pub fn new(code: Operation, operands: [usize; 3]) -> Self {
        Self {
            code,
            operands,
            metadata: Metadata::default(),
            r#type: 0,
        }
    }

    pub fn with_metadata(&mut self, metadata: Metadata) -> &mut Self {
        self.metadata = metadata;

        self
    }

    #[must_use]
    pub fn operands(&self) -> &[usize; 3] {
        self.operands.borrow()
    }

    pub fn operands_mut(&mut self) -> &mut [usize; 3] {
        self.operands.borrow_mut()
    }

    pub fn attach_metadata(&mut self, metadata: Metadata) {
        self.metadata = metadata;
    }

    #[must_use]
    pub fn code(&self) -> Operation {
        self.code
    }

    #[must_use]
    pub fn get(&self, idx: usize) -> usize {
        self.operands[idx]
    }

    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn set_type(&mut self, r#type: usize) {
        self.r#type = r#type;
    }

    #[must_use]
    pub fn kind(&self) -> usize {
        self.r#type
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Byte {
    /// Terminate the execution of the program (unrecoverable)
    Halt,
    /// Label a position in the bytecode
    Label,
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
    /// Jump to absolute offset
    Jump,
    /// Jump to relative offset
    Jumpr,
    /// Conditionally jump to relative offset
    Jumpz,
    /// Flip the expression
    Not,
    /// Negate
    Negate,
    /// Concatenate 2 strings
    Concat,

    /// Sum 2 values
    Add,
    /// Subtract,
    Sub,
    /// Multiplication,
    Mul,
    /// Division
    Div,
    /// Modulo
    Mod,
    /// Raise $lhs to the power of $rhs
    Pow,
    /// Left Shift
    LShift,
    /// Right Shift
    RShift,
    /// Xor
    Xor,
    /// Bit And
    BAnd,
    /// Bit Or
    BOr,
    /// And
    And,
    /// Or
    Or,
    /// Less than
    Less,
    /// Less or equal
    LessEqual,
    /// Greater than
    Greater,
    /// Greater or equal
    GreaterEqual,
    /// Equal
    Equal,
    /// Push a value on the stack
    Push,
    /// Pop a value from the stack
    Pop,
    /// Return item at offset from the stack top
    Peek,
    /// Duplicate the last value on the stack
    Duplicate,
    /// Prints the value from the top of the stack
    Print,
    /// Call a native module function
    Native,
    /// Call a function
    Call,
    /// Load a variable
    Load,
    /// Store a variable
    Store,
    /// Upvalue
    Upvalue,
    /// Class
    Class,
    /// Property
    Prop,
    /// Method
    Method,
    /// Instantiate
    Instantiate,
    /// Invoke method
    Invoke,
    ///
    Iterator,
    /// ??
    Iterate,
    /// This
    This,
    /// Gets the type of the last item on the stack
    TypeOf,
    /// Yield from the current function, suspending it
    Yield,

    /// Range of numerical values
    Range,
    /// Array of elements
    Array,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Code {
    byte: Byte,
    operands: [usize; 3],
    r#type: usize,
}

impl Code {
    #[must_use]
    pub fn new(byte: Byte) -> Self {
        Self {
            byte,
            operands: [0, 0, 0],
            r#type: 0,
        }
    }

    #[must_use]
    pub fn new_with_operands(byte: Byte, operands: [usize; 3]) -> Self {
        Self {
            byte,
            operands,
            r#type: 0,
        }
    }

    pub fn with_operands(&mut self, operands: [usize; 3]) {
        self.operands = operands;
    }

    pub fn with_type(&mut self, r#type: usize) {
        self.r#type = r#type;
    }

    #[must_use]
    pub fn byte(&self) -> &Byte {
        &self.byte
    }

    #[must_use]
    pub fn operand(&self, idx: usize) -> usize {
        self.operands[idx]
    }

    #[must_use]
    pub fn operands(&self) -> &[usize; 3] {
        &self.operands
    }

    #[must_use]
    pub fn get_type(&self) -> usize {
        self.r#type
    }
}
