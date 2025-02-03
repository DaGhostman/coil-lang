use std::ops::{Add, Rem, Shl, Sub};

use common::{
    opcodes::{Code, Operation, IR},
    Value, ValueKind,
};
use parser::Program;

use crate::CompilationPass;

#[derive(Default)]
pub struct ConstantFolding {}

impl CompilationPass for ConstantFolding {
    fn compile(
        &mut self,
        code: parser::Program<common::opcodes::IR>,
    ) -> Result<parser::Program<common::opcodes::IR>, common::error::Error> {
        let mut constants = code.constants();
        let mut modified: Vec<IR> = vec![];
        let mut phase = code.code();

        loop {
            let mut cursor = 0;
            modified = vec![];

            while let Some(op) = phase.get(cursor) {
                match op.code() {
                    Operation::Add => {
                        let rhs = modified.get(modified.len() - 1);
                        let lhs = modified.get(modified.len() - 2);

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants.intern(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_add(rhs)),
                                    ));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.add(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants.intern(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_sub(rhs)),
                                    ));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants.intern(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_mul(rhs)),
                                    ));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants.intern(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_div(rhs)),
                                    ));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.sub(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants.intern(Value::new(
                                        ValueKind::INTEGER(lhs.wrapping_rem(rhs)),
                                    ));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.rem(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant =
                                        constants.intern(Value::new(ValueKind::INTEGER(
                                            lhs.wrapping_pow((rhs).try_into().unwrap_or_default()),
                                        )));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::FLOAT(lhs.powf(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
                                }
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                                    let constant = constants.intern(Value::new(ValueKind::FLOAT(
                                        lhs.powi((rhs).try_into().unwrap_or_default()),
                                    )));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::BOOLEAN(lhs == rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::BOOLEAN(lhs != rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant =
                                        constants.intern(Value::new(ValueKind::BOOLEAN(lhs < rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::BOOLEAN(lhs <= rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant =
                                        constants.intern(Value::new(ValueKind::BOOLEAN(lhs > rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(rhs), Some(lhs)) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::BOOLEAN(lhs >= rhs)));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant = constants
                                        .intern(Value::new(ValueKind::INTEGER(lhs.shl(rhs))));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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

                        if let (Some(Operation::Push), Some(Operation::Push)) =
                            (rhs.map(|c| c.code()), lhs.map(|c| c.code()))
                        {
                            // Here we pop so we can remove the values
                            // this heavily relies that the type-checker
                            // has covered all the cases and the folding
                            // will not result in any type-errors
                            match (
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                                modified.pop().map(|c| {
                                    constants
                                        .lookup(c.get(0).copied().unwrap_or_default())
                                        .copied()
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                }),
                            ) {
                                (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                                    let constant =
                                        constants.intern(Value::new(ValueKind::INTEGER(
                                            lhs.wrapping_shr((rhs).try_into().unwrap_or_default()),
                                        )));
                                    modified.push(IR::new(Operation::Push, Some([constant, 0, 0])));
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
                            (Some(Operation::Push), Some(Operation::Push)) => {
                                let rhs = modified.get(last).map(|ir| {
                                    ir.get(0)
                                        .map(|c| code.constant(*c).copied().unwrap_or_default())
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
                                });
                                let lhs = modified.get(last - 1).map(|ir| {
                                    ir.get(0)
                                        .map(|c| code.constant(*c).copied().unwrap_or_default())
                                        .unwrap_or_default()
                                        .kind()
                                        .clone()
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
                                            Operation::Push,
                                            Some([
                                                constants
                                                    .intern(Value::new(ValueKind::RANGE(lhs, rhs))),
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

            if modified.len() == phase.len() && modified == phase {
                break;
            }

            phase = modified.clone();
        }

        Ok(Program::new(
            modified,
            constants,
            code.strings(),
            code.symbols(),
        ))
    }
}
