//! Optional Cranelift backend for the first JIT prototype.
//!
//! The default build keeps the crate dependency-free and reports that JIT is
//! disabled. Enable `cranelift` to compile small pure numeric functions.

use std::fmt::{Display, Formatter};

mod bytecode;
mod policy;

pub use bytecode::translate_i64_bytecode;
pub use policy::{HotCounters, JitConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JitError {
    Disabled,
    InvalidIr(String),
    Backend(String),
}

impl Display for JitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("coil-jit was built without the `cranelift` feature"),
            Self::InvalidIr(message) => write!(f, "invalid JIT IR: {message}"),
            Self::Backend(message) => write!(f, "Cranelift backend error: {message}"),
        }
    }
}

impl std::error::Error for JitError {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I64BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum F64BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JitValueKind {
    I64,
    F64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum I64CompareOp {
    Less,
    LessEqual,
    Equal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum I64Instr {
    LoadParam {
        dst: u8,
        param: u8,
    },
    Const {
        dst: u8,
        value: i64,
    },
    Binary {
        dst: u8,
        lhs: u8,
        rhs: u8,
        op: I64BinaryOp,
    },
    Compare {
        dst: u8,
        lhs: u8,
        rhs: u8,
        op: I64CompareOp,
    },
    Label {
        block: u32,
    },
    Jump {
        target: u32,
    },
    Branch {
        cond: u8,
        then_block: u32,
        else_block: u32,
    },
    CallSelf {
        dst: u8,
        args: Vec<u8>,
    },
    ArrayLen {
        dst: u8,
        value: u8,
    },
    ArrayIndex {
        dst: u8,
        array: u8,
        index: u8,
    },
    Return {
        value: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct I64Function {
    params: u8,
    instructions: Vec<I64Instr>,
}

impl I64Function {
    pub fn new(params: u8, instructions: Vec<I64Instr>) -> Self {
        Self {
            params,
            instructions,
        }
    }

    pub fn binary(op: I64BinaryOp) -> Self {
        Self::new(
            2,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::LoadParam { dst: 1, param: 1 },
                I64Instr::Binary {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                    op,
                },
                I64Instr::Return { value: 2 },
            ],
        )
    }

    pub fn binary_imm(op: I64BinaryOp, value: i64) -> Self {
        Self::new(
            1,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::Const { dst: 1, value },
                I64Instr::Binary {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                    op,
                },
                I64Instr::Return { value: 2 },
            ],
        )
    }

    pub fn array_len() -> Self {
        Self::new(
            1,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::ArrayLen { dst: 1, value: 0 },
                I64Instr::Return { value: 1 },
            ],
        )
    }

    pub fn array_index_const(index: i64) -> Self {
        Self::new(
            1,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::Const {
                    dst: 1,
                    value: index,
                },
                I64Instr::ArrayIndex {
                    dst: 2,
                    array: 0,
                    index: 1,
                },
                I64Instr::Return { value: 2 },
            ],
        )
    }

    pub fn uses_context(&self) -> bool {
        self.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                I64Instr::ArrayLen { .. } | I64Instr::ArrayIndex { .. }
            )
        })
    }

    #[cfg(any(feature = "cranelift", test))]
    pub(crate) fn validate(&self) -> Result<u8, JitError> {
        let mut defined = [false; 256];
        let mut max_register = 0u8;
        let mut saw_return = false;
        let mut terminated = false;
        let mut defined_labels = std::collections::HashSet::new();
        let mut referenced_labels = std::collections::HashSet::new();
        defined_labels.insert(0);

        for (index, instruction) in self.instructions.iter().enumerate() {
            if let I64Instr::Label { block } = instruction {
                if !defined_labels.insert(*block) {
                    return Err(JitError::InvalidIr(format!(
                        "duplicate block label {block}"
                    )));
                }
                terminated = false;
                continue;
            }
            if terminated {
                return Err(JitError::InvalidIr(format!(
                    "instruction after terminator at index {index}"
                )));
            }
            let define = |defined: &mut [bool; 256],
                          max_register: &mut u8,
                          dst: u8|
             -> Result<(), JitError> {
                if defined[dst as usize] {
                    return Err(JitError::InvalidIr(format!(
                        "register r{dst} is defined more than once"
                    )));
                }
                defined[dst as usize] = true;
                *max_register = (*max_register).max(dst);
                Ok(())
            };
            let use_register = |defined: &[bool; 256], register: u8| defined[register as usize];

            match instruction {
                I64Instr::LoadParam { dst, param } => {
                    if *param >= self.params {
                        return Err(JitError::InvalidIr(format!(
                            "parameter {param} is outside arity {}",
                            self.params
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::Const { dst, .. } => {
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::Binary { dst, lhs, rhs, .. } => {
                    if !use_register(&defined, *lhs) || !use_register(&defined, *rhs) {
                        return Err(JitError::InvalidIr(format!(
                            "binary instruction at index {index} uses an undefined register"
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::Compare { dst, lhs, rhs, .. } => {
                    if !use_register(&defined, *lhs) || !use_register(&defined, *rhs) {
                        return Err(JitError::InvalidIr(format!(
                            "compare instruction at index {index} uses an undefined register"
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::Jump { target } => {
                    referenced_labels.insert(*target);
                    terminated = true;
                }
                I64Instr::Branch {
                    cond,
                    then_block,
                    else_block,
                } => {
                    if !use_register(&defined, *cond) {
                        return Err(JitError::InvalidIr(format!(
                            "branch at index {index} uses an undefined register"
                        )));
                    }
                    referenced_labels.insert(*then_block);
                    referenced_labels.insert(*else_block);
                    terminated = true;
                }
                I64Instr::CallSelf { dst, args } => {
                    if args.len() != self.params as usize {
                        return Err(JitError::InvalidIr(format!(
                            "self-call at index {index} has {} args for arity {}",
                            args.len(),
                            self.params
                        )));
                    }
                    if args
                        .iter()
                        .any(|register| !use_register(&defined, *register))
                    {
                        return Err(JitError::InvalidIr(format!(
                            "self-call at index {index} uses an undefined register"
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::ArrayLen { dst, value } => {
                    if !use_register(&defined, *value) {
                        return Err(JitError::InvalidIr(format!(
                            "array length at index {index} uses an undefined register"
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::ArrayIndex {
                    dst,
                    array,
                    index: index_register,
                } => {
                    if !use_register(&defined, *array) || !use_register(&defined, *index_register) {
                        return Err(JitError::InvalidIr(format!(
                            "array index at index {index} uses an undefined register"
                        )));
                    }
                    define(&mut defined, &mut max_register, *dst)?;
                }
                I64Instr::Return { value } => {
                    if !use_register(&defined, *value) {
                        return Err(JitError::InvalidIr(format!(
                            "return at index {index} uses an undefined register"
                        )));
                    }
                    max_register = max_register.max(*value);
                    saw_return = true;
                    terminated = true;
                }
                I64Instr::Label { .. } => unreachable!("labels are handled before this match"),
            }
        }

        if !saw_return {
            return Err(JitError::InvalidIr("function has no return".into()));
        }
        if let Some(target) = referenced_labels
            .into_iter()
            .find(|target| !defined_labels.contains(target))
        {
            return Err(JitError::InvalidIr(format!(
                "branch targets undefined block {target}"
            )));
        }
        Ok(max_register.saturating_add(1))
    }

    pub fn params(&self) -> u8 {
        self.params
    }

    #[cfg(feature = "cranelift")]
    pub(crate) fn instructions(&self) -> &[I64Instr] {
        &self.instructions
    }
}

#[derive(Copy, Clone)]
pub struct JitHelpers {
    pub array_len: *const u8,
    pub array_index: *const u8,
}

impl Default for JitHelpers {
    fn default() -> Self {
        Self {
            array_len: std::ptr::null(),
            array_index: std::ptr::null(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum F64Instr {
    LoadParam {
        dst: u8,
        param: u8,
    },
    Const {
        dst: u8,
        value: f64,
    },
    Binary {
        dst: u8,
        lhs: u8,
        rhs: u8,
        op: F64BinaryOp,
    },
    Return {
        value: u8,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct F64Function {
    params: u8,
    instructions: Vec<F64Instr>,
}

impl F64Function {
    pub fn binary(op: F64BinaryOp) -> Self {
        Self {
            params: 2,
            instructions: vec![
                F64Instr::LoadParam { dst: 0, param: 0 },
                F64Instr::LoadParam { dst: 1, param: 1 },
                F64Instr::Binary {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                    op,
                },
                F64Instr::Return { value: 2 },
            ],
        }
    }

    #[cfg(any(feature = "cranelift", test))]
    pub(crate) fn validate(&self) -> Result<(), JitError> {
        let mut defined = [false; 256];
        let mut returned = false;
        for (index, instruction) in self.instructions.iter().enumerate() {
            if returned {
                return Err(JitError::InvalidIr(format!(
                    "instruction after return at index {index}"
                )));
            }
            let define = |dst: u8, defined: &mut [bool; 256]| -> Result<(), JitError> {
                if defined[dst as usize] {
                    return Err(JitError::InvalidIr(format!(
                        "register r{dst} is defined more than once"
                    )));
                }
                defined[dst as usize] = true;
                Ok(())
            };
            let is_defined = |register: u8| defined[register as usize];
            match instruction {
                F64Instr::LoadParam { dst, param } => {
                    if *param >= self.params {
                        return Err(JitError::InvalidIr(format!(
                            "parameter {param} is outside arity {}",
                            self.params
                        )));
                    }
                    define(*dst, &mut defined)?;
                }
                F64Instr::Const { dst, .. } => define(*dst, &mut defined)?,
                F64Instr::Binary { dst, lhs, rhs, .. } => {
                    if !is_defined(*lhs) || !is_defined(*rhs) {
                        return Err(JitError::InvalidIr(format!(
                            "binary instruction at index {index} uses an undefined register"
                        )));
                    }
                    define(*dst, &mut defined)?;
                }
                F64Instr::Return { value } => {
                    if !is_defined(*value) {
                        return Err(JitError::InvalidIr(format!(
                            "return at index {index} uses an undefined register"
                        )));
                    }
                    returned = true;
                }
            }
        }
        if returned {
            Ok(())
        } else {
            Err(JitError::InvalidIr("function has no return".into()))
        }
    }

    #[cfg(feature = "cranelift")]
    pub(crate) fn params(&self) -> u8 {
        self.params
    }

    #[cfg(feature = "cranelift")]
    pub(crate) fn instructions(&self) -> &[F64Instr] {
        &self.instructions
    }
}

#[cfg(feature = "cranelift")]
mod cranelift_backend;

#[cfg(feature = "cranelift")]
pub use cranelift_backend::{JitEngine, JitFunction};

#[cfg(not(feature = "cranelift"))]
pub struct JitEngine;

#[cfg(not(feature = "cranelift"))]
pub struct JitFunction;

#[cfg(not(feature = "cranelift"))]
impl JitEngine {
    pub fn new() -> Result<Self, JitError> {
        Err(JitError::Disabled)
    }

    pub fn compile_i64(
        &mut self,
        _name: &str,
        _function: &I64Function,
    ) -> Result<JitFunction, JitError> {
        Err(JitError::Disabled)
    }

    pub fn compile_i64_binary(
        &mut self,
        _name: &str,
        _op: I64BinaryOp,
    ) -> Result<JitFunction, JitError> {
        Err(JitError::Disabled)
    }

    pub fn compile_i64_binary_imm(
        &mut self,
        _name: &str,
        _op: I64BinaryOp,
        _value: i64,
    ) -> Result<JitFunction, JitError> {
        Err(JitError::Disabled)
    }

    pub fn compile_f64_binary(
        &mut self,
        _name: &str,
        _op: F64BinaryOp,
    ) -> Result<JitFunction, JitError> {
        Err(JitError::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_reports_disabled_backend() {
        #[cfg(not(feature = "cranelift"))]
        assert!(matches!(JitEngine::new(), Err(JitError::Disabled)));
    }

    #[cfg(feature = "cranelift")]
    #[test]
    fn compiles_and_calls_i64_add() {
        let mut engine = JitEngine::new().expect("native target");
        let function = engine
            .compile_i64_binary("add", I64BinaryOp::Add)
            .expect("compile add");
        assert_eq!(function.call2(20, 22), 42);
    }

    #[cfg(feature = "cranelift")]
    #[test]
    fn compiles_sub_and_mul() {
        let mut engine = JitEngine::new().expect("native target");
        let sub_result = {
            let sub = engine
                .compile_i64_binary("sub", I64BinaryOp::Sub)
                .expect("compile sub");
            sub.call2(44, 2)
        };
        let mul_result = {
            let mul = engine
                .compile_i64_binary("mul", I64BinaryOp::Mul)
                .expect("compile mul");
            mul.call2(6, 7)
        };
        assert_eq!(sub_result, 42);
        assert_eq!(mul_result, 42);
    }

    #[cfg(feature = "cranelift")]
    #[test]
    fn compiles_f64_binary() {
        let mut engine = JitEngine::new().expect("native target");
        let function = engine
            .compile_f64_binary("addf", F64BinaryOp::Add)
            .expect("compile float add");
        assert_eq!(function.call2_f64(20.5, 21.5), 42.0);
    }

    #[cfg(feature = "cranelift")]
    #[test]
    fn compiles_linear_ir_with_a_constant() {
        let function = I64Function::new(
            2,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::Const { dst: 1, value: 1 },
                I64Instr::Binary {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                    op: I64BinaryOp::Add,
                },
                I64Instr::Return { value: 2 },
            ],
        );
        let mut engine = JitEngine::new().expect("native target");
        let compiled = engine
            .compile_i64("add_one", &function)
            .expect("compile linear IR");
        assert_eq!(compiled.call2(41, 0), 42);
    }

    #[cfg(feature = "cranelift")]
    #[test]
    fn compiles_self_recursive_fibonacci() {
        let function = I64Function::new(
            1,
            vec![
                I64Instr::LoadParam { dst: 0, param: 0 },
                I64Instr::Const { dst: 1, value: 1 },
                I64Instr::Compare {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                    op: I64CompareOp::LessEqual,
                },
                I64Instr::Branch {
                    cond: 2,
                    then_block: 1,
                    else_block: 2,
                },
                I64Instr::Label { block: 1 },
                I64Instr::Const { dst: 3, value: 1 },
                I64Instr::Return { value: 3 },
                I64Instr::Label { block: 2 },
                I64Instr::Const { dst: 4, value: 1 },
                I64Instr::Binary {
                    dst: 5,
                    lhs: 0,
                    rhs: 4,
                    op: I64BinaryOp::Sub,
                },
                I64Instr::CallSelf {
                    dst: 6,
                    args: vec![5],
                },
                I64Instr::Const { dst: 7, value: 2 },
                I64Instr::Binary {
                    dst: 8,
                    lhs: 0,
                    rhs: 7,
                    op: I64BinaryOp::Sub,
                },
                I64Instr::CallSelf {
                    dst: 9,
                    args: vec![8],
                },
                I64Instr::Binary {
                    dst: 10,
                    lhs: 6,
                    rhs: 9,
                    op: I64BinaryOp::Add,
                },
                I64Instr::Return { value: 10 },
            ],
        );
        let mut engine = JitEngine::new().expect("native target");
        let compiled = engine
            .compile_i64("fib", &function)
            .expect("compile recursive function");
        assert_eq!(compiled.call1(10), 89);
    }

    #[test]
    fn rejects_ir_without_return() {
        let function = I64Function::new(1, vec![I64Instr::Const { dst: 0, value: 1 }]);
        assert!(matches!(
            function.validate(),
            Err(JitError::InvalidIr(message)) if message.contains("no return")
        ));
    }
}
