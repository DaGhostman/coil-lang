use std::ops::{Add, Rem, Shl, Sub};

use common::interner::Interner;
use common::opcodes::{Byte, Code};
use common::program::Program;
use common::symbols::SymbolTable;
use common::{
    Value, ValueKind,
    opcodes::{IR, Operation},
};

use crate::CompilationPass;

#[derive(Default)]
pub struct ConstantFolding {}

impl CompilationPass for ConstantFolding {
    fn compile<'pass>(
        &mut self,
        code: &'pass Vec<Code>,
        constants: &mut Interner<Value>,
        symbols: &mut SymbolTable,
    ) -> Result<Vec<Code>, common::error::Error> {
        let default_value = Value::default();
        let mut length = code.len();

        loop {
            let mut modified: Vec<Code> = Vec::with_capacity(code.len());

            let mut cursor = 0;
            while let Some(op) = code.get(cursor) {
                cursor += 1;

                match op.byte() {
                    Byte::Label => {
                        if let Some(offset) = op.operand(1) {
                            if let Ok(mut chunk) = self.compile(
                                &code[cursor..cursor + offset].to_vec(),
                                constants,
                                symbols,
                            ) {
                                modified.push(Code::new_with_operands(
                                    Byte::Label,
                                    vec![op.operand(0).unwrap(), chunk.len()],
                                ));
                                modified.append(&mut chunk);
                            }
                            cursor += offset;
                        }
                    }
                    Byte::Add => {
                        let rhs = modified.last();
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Byte::Push), Some(Byte::Push)) =
                            (rhs.map(|o| o.byte()), lhs.map(|o| o.byte()))
                        {
                            match (
                                lhs.map(|c| {
                                    constants
                                        .lookup(c.operand(0).unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                rhs.map(|c| {
                                    constants
                                        .lookup(c.operand(0).unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(lhs)), Some(ValueKind::INTEGER(rhs))) => {
                                    let c =
                                        constants.intern(Value::new(ValueKind::INTEGER(lhs + rhs)));
                                    modified.pop();
                                    modified.pop();
                                    modified.push(Code::new_with_operands(Byte::Push, vec![c]));
                                }
                                (Some(ValueKind::FLOAT(lhs)), Some(ValueKind::FLOAT(rhs))) => {
                                    let c =
                                        constants.intern(Value::new(ValueKind::FLOAT(lhs + rhs)));
                                    modified.pop();
                                    modified.pop();
                                    modified.push(Code::new_with_operands(Byte::Push, vec![c]));
                                }
                                (Some(ValueKind::INTEGER(lhs)), Some(ValueKind::FLOAT(rhs))) => {
                                    let c = constants
                                        .intern(Value::new(ValueKind::FLOAT((*lhs as f64) + rhs)));
                                    modified.pop();
                                    modified.pop();
                                    modified.push(Code::new_with_operands(Byte::Push, vec![c]));
                                }
                                (Some(ValueKind::FLOAT(lhs)), Some(ValueKind::INTEGER(rhs))) => {
                                    let c = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs + (*rhs as f64))));
                                    modified.pop();
                                    modified.pop();
                                    modified.push(Code::new_with_operands(Byte::Push, vec![c]));
                                }
                                (a, b) => {
                                    // dbg!("NONO", &a, &b);
                                    modified.push(op.to_owned());
                                }
                            }
                        } else {
                            modified.push(op.to_owned());
                        }
                    }
                    _ => {
                        modified.push(op.to_owned());
                    }
                }
            }

            //
            // loop {
            //     let mut cursor = 0;
            //     modified = vec![];
            //
            //     while let Some(op) = code.get(cursor) {
            //         match op.code() {
            //             Operation::Add => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code.add_constant(Value::new(
            //                                 ValueKind::INTEGER(lhs.wrapping_add(*rhs)),
            //                             ));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                             modified.push(IR::new(Operation::Noop, None));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.add(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                             modified.push(IR::new(Operation::Noop, None));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Subtract => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code.add_constant(Value::new(
            //                                 ValueKind::INTEGER(lhs.wrapping_sub(*rhs)),
            //                             ));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Multiply => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code.add_constant(Value::new(
            //                                 ValueKind::INTEGER(lhs.wrapping_mul(*rhs)),
            //                             ));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Divide => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code.add_constant(Value::new(
            //                                 ValueKind::INTEGER(lhs.wrapping_div(*rhs)),
            //                             ));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Modulo => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code.add_constant(Value::new(
            //                                 ValueKind::INTEGER(lhs.wrapping_rem(*rhs)),
            //                             ));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.rem(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Pow => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant =
            //                                 code.add_constant(Value::new(ValueKind::INTEGER(
            //                                     lhs.wrapping_pow((*rhs).try_into().unwrap_or_default()),
            //                                 )));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::FLOAT(lhs.powf(*rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::FLOAT(lhs))) => {
            //                             let constant = code.add_constant(Value::new(ValueKind::FLOAT(
            //                                 lhs.powi((*rhs).try_into().unwrap_or_default()),
            //                             )));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Equal => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs == rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::NotEqual => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs != rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Less => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs < rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::LessEqual => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs <= rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::Greater => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs > rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::GreaterEqual => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(rhs), Some(lhs)) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::BOOLEAN(lhs >= rhs)));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::LeftShift => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant = code
            //                                 .add_constant(Value::new(ValueKind::INTEGER(lhs.shl(rhs))));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::RightShift => {
            //                 let rhs = modified.get(modified.len() - 1);
            //                 let lhs = modified.get(modified.len() - 2);
            //
            //                 if let (Some(Operation::Const), Some(Operation::Const)) =
            //                     (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
            //                 {
            //                     // Here we pop so we can remove the values
            //                     // this heavily relies that the type-checker
            //                     // has covered all the cases and the folding
            //                     // will not result in any type-errors
            //                     match (
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                         modified.pop().map(|c| {
            //                             constants.lookup(c.get(0).copied().unwrap_or_default())
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         }),
            //                     ) {
            //                         (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
            //                             let constant =
            //                                 code.add_constant(Value::new(ValueKind::INTEGER(
            //                                     lhs.wrapping_shr((*rhs).try_into().unwrap_or_default()),
            //                                 )));
            //                             modified
            //                                 .push(IR::new(Operation::Const, Some([constant, 0, 0])));
            //                         }
            //                         _ => modified.push(*op),
            //                     }
            //                 } else {
            //                     modified.push(*op);
            //                 }
            //             }
            //             Operation::ConditionJump => {
            //                 eprintln!("Handle JUMP elimination");
            //                 modified.push(*op);
            //             }
            //             Operation::Range => {
            //                 let last = modified.len() - 1;
            //                 match (
            //                     modified.get(last).map(|c| c.code()),
            //                     modified.get(last - 1).map(|c| c.code()),
            //                 ) {
            //                     (Some(Operation::Const), Some(Operation::Const)) => {
            //                         let rhs = modified.get(last).map(|ir| {
            //                             ir.get(0)
            //                                 .map(|c| constants.lookup(*c).unwrap_or(&default_value))
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         });
            //                         let lhs = modified.get(last - 1).map(|ir| {
            //                             ir.get(0)
            //                                 .map(|c| constants.lookup(*c).unwrap_or(&default_value))
            //                                 .unwrap_or(&default_value)
            //                                 .kind()
            //                         });
            //
            //                         match (lhs, rhs) {
            //                             (
            //                                 Some(ValueKind::INTEGER(lhs)),
            //                                 Some(ValueKind::INTEGER(rhs)),
            //                             ) => {
            //                                 // drop the ranges for a single value range
            //                                 modified.pop();
            //                                 modified.pop();
            //
            //                                 modified.push(IR::new(
            //                                     Operation::Const,
            //                                     Some([
            //                                         code.add_constant(Value::new(ValueKind::RANGE(
            //                                             *lhs, *rhs,
            //                                         ))),
            //                                         0,
            //                                         0,
            //                                     ]),
            //                                 ));
            //                             }
            //                             _ => (),
            //                         }
            //                     }
            //                     _ => (),
            //                 }
            //             }
            //             _ => {
            //                 modified.push(*op);
            //             }
            //         }
            //         cursor += 1;
            //     }
            if length == modified.len() {
                return Ok(modified);
            }
            length = modified.len();
        }
    }
}
