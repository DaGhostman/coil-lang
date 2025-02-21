use std::ops::{Add, Rem, Shl, Sub};

use common::program::Program;
use common::{
    opcodes::{Operation, IR},
    Value, ValueKind,
};

use crate::CompilationPass;

#[derive(Default)]
pub struct ConstantFolding {}

impl CompilationPass for ConstantFolding {
    fn compile<'pass>(
        &mut self,
        code: &'pass mut Program<common::opcodes::IR>,
    ) -> Result<&'pass mut Program<common::opcodes::IR>, common::error::Error> {
        let mut modified: Vec<IR>;
        let default_value = Value::default();

        loop {
            let mut cursor = 0;
            modified = vec![];

            while let Some(op) = code.get(cursor) {
                match op.code() {
                    Operation::Add => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code.add_constant(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_add(*rhs)),
                                    ));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.add(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Subtract => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code.add_constant(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_sub(*rhs)),
                                    ));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Multiply => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code.add_constant(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_mul(*rhs)),
                                    ));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Divide => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code.add_constant(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_div(*rhs)),
                                    ));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Modulo => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code.add_constant(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_rem(*rhs)),
                                    ));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.rem(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Pow => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant =
                                        code.add_constant(Value::new(ValueKind::INTEGER(
                                            lhs.wrapping_pow((*rhs).try_into().unwrap_or_default()),
                                        )));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::FLOAT(lhs.powf(*rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = code.add_constant(Value::new(ValueKind::FLOAT(
                                        lhs.powi((*rhs).try_into().unwrap_or_default()),
                                    )));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Equal => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs == rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::NotEqual => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs != rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Less => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs < rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::LessEqual => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs <= rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::Greater => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs > rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::GreaterEqual => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::BOOLEAN(lhs >= rhs)));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::LeftShift => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = code
                                        .add_constant(Value::new(ValueKind::INTEGER(lhs.shl(rhs))));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::RightShift => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Const), Some(Operation::Const)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                                modified.pop().map(|c| {
                                    code.constant(c.get(0).copied().unwrap_or_default())
                                        .unwrap_or(&default_value)
                                        .kind()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant =
                                        code.add_constant(Value::new(ValueKind::INTEGER(
                                            lhs.wrapping_shr((*rhs).try_into().unwrap_or_default()),
                                        )));
                                    modified
                                        .push(IR::new(Operation::Const, Some([constant, 0, 0])));
                                }
                                _ => modified.push(*op),
                            }
                        } else {
                            modified.push(*op);
                        }
                    }
                    Operation::ConditionJump => {
                        eprintln!("Handle JUMP elimination");
                        modified.push(*op);
                    }
                    Operation::Range => {
                        let last = modified.len() - 1;
                        match (
                            modified.get(last).map(|c| c.code()),
                            modified.get(last - 1).map(|c| c.code()),
                        ) {
                            (Some(Operation::Const), Some(Operation::Const)) => {
                                let rhs = modified.get(last).map(|ir| {
                                    ir.get(0)
                                        .map(|c| code.constant(*c).unwrap_or(&default_value))
                                        .unwrap_or(&default_value)
                                        .kind()
                                });
                                let lhs = modified.get(last - 1).map(|ir| {
                                    ir.get(0)
                                        .map(|c| code.constant(*c).unwrap_or(&default_value))
                                        .unwrap_or(&default_value)
                                        .kind()
                                });

                                match (lhs, rhs) {
                                    (
                                        Some(ValueKind::INTEGER(lhs)),
                                        Some(ValueKind::INTEGER(rhs)),
                                    ) => {
                                        // drop the ranges for a single value range
                                        modified.pop();
                                        modified.pop();

                                        modified.push(IR::new(
                                            Operation::Const,
                                            Some([
                                                code.add_constant(Value::new(ValueKind::RANGE(
                                                    *lhs, *rhs,
                                                ))),
                                                0,
                                                0,
                                            ]),
                                        ));
                                    }
                                    _ => (),
                                }
                            }
                            _ => (),
                        }
                    }
                    _ => {
                        modified.push(*op);
                    }
                }
                cursor += 1;
            }

            if code.with_code(modified) {
                break;
            }
        }
        Ok(code)
    }
}
