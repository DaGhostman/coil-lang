use common::Value;
use common::opcodes::{Byte, Code};
use common::program::data::Data;
use common::types::{Kind, Type};

use crate::CompilationPass;

#[derive(Default)]
pub struct ConstantFolding {}

impl CompilationPass for ConstantFolding {
    fn compile(
        &mut self,
        code: &[Code],
        data: &mut Data,
    ) -> Result<Vec<Code>, Vec<common::error::Message>> {
        let mut code = code.to_vec();
        let mut length = code.len();
        let mut modified: Vec<Code> = Vec::with_capacity(length);

        macro_rules! binary {
            ($lhs:expr, $rhs:expr, $op:tt) => {
                {
                        let result = Value::from($lhs $op $rhs);
                        let r#type = data.find_type(Type::new(result.into()));
                        let result = data.add_constant(result, r#type);

                        modified.pop();
                        modified.pop();
                        modified.push(Code::new_with_operands(Byte::Push, [result, 0, 0]));
                }
            };
        }

        macro_rules! unary {
            ($rhs:expr, $op:tt) => {
                {
                    let result = Value::from($op $rhs);
                        let r#type = data.find_type(Type::new(result.into()));
                        let result = data.add_constant(result, r#type);

                        modified.pop();
                        modified.push(Code::new_with_operands(Byte::Push, [result, 0, 0]));
                }
            }
        }

        macro_rules! comparison {
            ($lhs:expr, $rhs: expr, $op: tt, $current:expr) => {{
                if *$lhs.byte() == Byte::Push && *$rhs.byte() == Byte::Push {
                    let r#type = data.add_type(Type::bool());
                    let result = data.add_constant(Value::from(data.constant($lhs.operand(0)) $op data.constant($rhs.operand(0))), r#type);

                    modified.pop();
                    modified.pop();
                    modified.push(Code::new_with_operands(Byte::Push, [result, 0, 0]));
                } else {
                    modified.push(*$current);
                }
            }};
        }

        macro_rules! math {
            ($lhs:expr, $rhs: expr, $op: tt, $current:expr) => {{
                if *$lhs.byte() == Byte::Push && *$rhs.byte() == Byte::Push {
                    let l = data.constant($lhs.operand(0)).to_owned();
                    let r = data.constant($rhs.operand(0)).to_owned();

                    match (l, r) {
                        (Value::INTEGER(lhs), Value::INTEGER(rhs)) => binary!(lhs, rhs, $op),
                        (Value::FLOAT(lhs), Value::FLOAT(rhs)) => binary!(lhs, rhs, $op),
                        (Value::INTEGER(lhs), Value::FLOAT(rhs)) => binary!(lhs as f64, rhs, $op),
                        (Value::FLOAT(lhs), Value::INTEGER(rhs)) => binary!(lhs, rhs as f64, $op),
                        (Value::STR(lhs), Value::STR(rhs)) => {
                            let r#type = data.add_type(Type::string());
                            let concatenated = data.add_string(format!(
                                "{}{}",
                                data.string(lhs),
                                data.string(rhs)
                            ));
                            let result = data.add_constant(Value::STR(concatenated), r#type);

                            modified.push(Code::new_with_operands(Byte::Push, [result, 0, 0]));
                        }
                        _ => {
                            modified.push(*$current);
                        }
                    }
                } else {
                    modified.push(*$current);
                }
            }};
        }
        macro_rules! bitwise {
            ($lhs:expr, $rhs: expr, $op: tt, $current:expr) => {{
                if *$lhs.byte() == Byte::Push && *$rhs.byte() == Byte::Push {
                    match (
                        data.constant($lhs.operand(0)),
                        data.constant($rhs.operand(0)),
                    ) {
                        (Value::INTEGER(lhs), Value::INTEGER(rhs)) => binary!(lhs, rhs, $op),
                        _ => {
                            modified.push(*$current);
                        }
                    }
                } else {
                    modified.push(*$current);
                }
            }};
        }

        macro_rules! prefix {
            ($rhs:expr, $op: tt, $current:expr) => {{
                if *$rhs.byte() == Byte::Push {
                    match data.constant($rhs.operand(0)) {
                        Value::INTEGER(rhs) => unary!(rhs, $op),
                        _ => {
                            modified.push(*$current);
                        }
                    }
                } else {
                    modified.push(*$current);
                }
            }};
        }

        loop {
            let mut cursor = 0;
            while let Some(op) = code.get(cursor) {
                cursor += 1;

                match op.byte() {
                    Byte::Label => {
                        modified.push(*op);
                        let offset = op.operand(1);
                        if let Ok(mut chunk) = self.compile(&code[cursor..cursor + offset], data) {
                            modified.push(Code::new_with_operands(
                                Byte::Label,
                                [op.operand(0), chunk.len(), 0],
                            ));
                            modified.append(&mut chunk);
                        }
                        cursor += offset;
                    }
                    Byte::TypeOf => {
                        let prev = modified.last();

                        if let Some((Byte::Push, ty)) =
                            prev.map(|code| (code.byte(), code.get_type()))
                        {
                            modified.pop();

                            let val = Value::TYPE(ty);
                            let ty = data.add_type(Type::new(Kind::Type));
                            let constant = data.add_constant(val, ty);

                            modified.pop();
                            modified.push(Code::new_with_operands(Byte::Push, [constant, 0, 0]));
                        } else {
                            modified.push(*op);
                        }
                    }
                    Byte::Negate => {
                        let rhs = modified[modified.len() - 1];

                        prefix!(rhs, -, op);
                    }
                    Byte::Not => {
                        let rhs = modified[modified.len() - 1];

                        prefix!(rhs, !, op);
                    }
                    Byte::Add => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        math!(lhs, rhs, +, op);
                    }
                    Byte::Sub => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        math!(lhs, rhs, -, op);
                    }
                    Byte::Mul => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        math!(lhs, rhs, *, op);
                    }
                    Byte::Div => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        math!(lhs, rhs, /, op);
                    }
                    Byte::Mod => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        math!(lhs, rhs, %, op);
                    }
                    Byte::Pow => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        if *lhs.byte() == Byte::Push && *rhs.byte() == Byte::Push {
                            let lhs = *data.constant(lhs.operand(0));
                            let rhs = *data.constant(rhs.operand(0));

                            let result = match (lhs, rhs) {
                                (Value::INTEGER(lhs), Value::INTEGER(rhs)) => {
                                    modified.pop();
                                    modified.pop();
                                    Value::INTEGER(lhs.pow(rhs as u32))
                                }
                                (Value::FLOAT(lhs), Value::INTEGER(rhs)) => {
                                    modified.pop();
                                    modified.pop();
                                    Value::FLOAT(lhs.powf(rhs as f64))
                                }
                                (Value::INTEGER(lhs), Value::FLOAT(rhs)) => {
                                    modified.pop();
                                    modified.pop();
                                    Value::INTEGER(lhs.pow(rhs as u32))
                                }
                                (Value::FLOAT(lhs), Value::FLOAT(rhs)) => {
                                    modified.pop();
                                    modified.pop();
                                    Value::FLOAT(lhs.powf(rhs))
                                }
                                _ => {
                                    modified.pop();
                                    modified.pop();

                                    Value::NONE
                                }
                            };

                            let r#type = data.add_type(Type::new(result.into()));
                            let result = data.add_constant(result, r#type);

                            modified.push(Code::new_with_operands(Byte::Push, [result, 0, 0]));
                        } else {
                            modified.push(*op);
                        }
                    }
                    Byte::LShift => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        bitwise!(lhs, rhs, <<, op);
                    }
                    Byte::RShift => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        bitwise!(lhs, rhs, >>, op);
                    }
                    Byte::BOr => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        bitwise!(lhs, rhs, |, op);
                    }
                    Byte::BAnd => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        bitwise!(lhs, rhs, &, op);
                    }
                    Byte::Xor => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        bitwise!(lhs, rhs, ^, op);
                    }
                    Byte::Less => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        comparison!(lhs, rhs, <, op);
                    }
                    Byte::LessEqual => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        comparison!(lhs, rhs, <=, op);
                    }
                    Byte::Equal => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        comparison!(lhs, rhs, ==, op);
                    }
                    Byte::GreaterEqual => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        comparison!(lhs, rhs, >=, op);
                    }
                    Byte::Greater => {
                        let rhs = modified[modified.len() - 1];
                        let lhs = modified[modified.len() - 2];

                        comparison!(lhs, rhs, >, op);
                    }
                    _ => {
                        modified.push(*op);
                    }
                }
            }

            length = modified.len().to_owned();
            if length == modified.len() {
                return Ok(modified);
            }

            code = modified.clone();
            modified.drain(0..);
        }
    }
}
