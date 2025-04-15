use common::{
    Value,
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
    program::data::Data,
    types::{Kind, Type},
};

use rustc_hash::FxHashMap as HashMap;

use crate::CompilationPass;

#[derive(Debug)]
pub struct TypeChecker<const N: usize> {
    stack: [Type; N],
    sp: usize,
    errors: Vec<Error>,
}

impl<const N: usize> Default for TypeChecker<N> {
    fn default() -> Self {
        Self {
            sp: 0,
            stack: [Type::default(); N],
            errors: Vec::with_capacity(8),
        }
    }
}

impl<const N: usize> TypeChecker<N> {
    fn push(&mut self, type_: Type) {
        self.stack[self.sp] = type_;
        self.sp += 1;
    }

    fn pop(&mut self, n: usize) -> Type {
        let sp = self.sp;
        self.sp -= n;
        self.stack[sp]
    }

    fn peek(&self, offset: usize) -> Type {
        self.stack[self.sp - 1 - offset]
    }

    fn last(&self) -> Type {
        self.stack[self.sp - 1]
    }

    fn do_compile(&mut self, code: &[Code], data: &mut Data) -> Vec<Code> {
        let mut bytecode = vec![];
        let mut ip = 0;

        let mut variables = HashMap::default();

        while ip < code.len() {
            let op = code[ip];

            match op.byte() {
                Byte::Push => {
                    self.push(Type::new(Kind::from(*data.constant(op.operand(0)))));
                }
                Byte::Pop => {
                    self.pop(op.operand(0));
                }
                Byte::Add | Byte::Sub | Byte::Mul | Byte::Div => {
                    let type_ = match (self.pop(1).kind(), self.pop(1).kind()) {
                        (Kind::Integer, Kind::Integer) => Kind::Integer,
                        (Kind::Float, Kind::Float) => Kind::Float,
                        (Kind::Integer, Kind::Float) => Kind::Float,
                        (Kind::Float, Kind::Integer) => Kind::Float,
                        (r, l) => {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!(
                                    "{:?} {:?} {:?} has invalid types and is therefore not allowed",
                                    l,
                                    op.byte(),
                                    r
                                ),
                            ));
                            Kind::None
                        }
                    };

                    self.push(Type::new(type_));
                }
                Byte::Less | Byte::LessEqual | Byte::Equal | Byte::Greater | Byte::GreaterEqual => {
                    let r = self.pop(1);
                    let l = self.pop(1);

                    match (l.kind(), r.kind()) {
                        (Kind::Integer, Kind::Integer) => (),
                        (Kind::Float, Kind::Float) => (),
                        (Kind::Integer, Kind::Float) => (),
                        (Kind::Float, Kind::Integer) => (),
                        (Kind::String, Kind::String) => (),
                        (Kind::Object(_), Kind::Object(_)) => (),
                        (Kind::List(_), Kind::List(_)) => (),
                        _ => {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!("Unable to do a comparison between {:?} and {:?}", l, r),
                            ));
                        }
                    }

                    self.push(Type::new(Kind::Bool));
                }
                Byte::Jumpz => {
                    self.pop(1);
                }
                Byte::Store => {
                    variables.insert(op.operand(0), self.pop(1));
                }
                Byte::Call => {
                    self.push(op.get_type());
                }
                Byte::Load => {
                    self.push(
                        variables
                            .get(&op.operand(0))
                            .copied()
                            .unwrap_or(op.get_type()),
                    );
                    // self.push(variables[&op.operand(0)]);
                    // self.push(if let Some(kind) = op.kind() {
                    //     kind
                    // } else {
                    //     Type::None
                    // });
                }
                Byte::Instantiate => {
                    self.push(op.get_type());
                }
                // Byte::This => {
                //     self.push(if let Some(kind) = op.kind() {
                //         kind
                //     } else {
                //         Type::None
                //     });
                // }
                Byte::Invoke => {
                    if !matches!(self.peek(op.operand(1)).kind(), Kind::Object(_)) {
                        self.errors.push(Error::new(
                            ErrorOrigin::COMPILE,
                            "Unable to invoke a method on non-object".to_string(),
                        ));
                    }
                }
                _ => (),
            };

            bytecode.push(op);
            ip += 1;
        }

        bytecode
    }
}

impl<const N: usize> CompilationPass for TypeChecker<N> {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        data: &mut Data,
    ) -> Result<Vec<common::opcodes::Code>, Error> {
        let code = self.do_compile(code, data);

        if !self.errors.is_empty() {
            let error = (Error::new(
                ErrorOrigin::COMPILE,
                "Unable to finish compilation due to the following type errors".to_string(),
            ));

            for error in &self.errors {
                eprintln!("{}", error);
            }

            return Err(error);
        }

        Ok(code)
        // let mut variables: HashMap<usize, Type> = HashMap::new();
        //
        // for op in program.code() {
        //     match op.code() {
        //         Operation::Const => {
        //             match program
        //                 .constant(op.get(0).copied().unwrap_or_default())
        //                 .map(|v| v.kind())
        //             {
        //                 Some(ValueType::BOOLEAN(_)) => self.types.push(Type::Bool),
        //                 Some(ValueType::INTEGER(_)) => self.types.push(Type::Integer),
        //                 Some(ValueType::FLOAT(_)) => self.types.push(Type::Float),
        //                 Some(ValueType::STRING(_)) => self.types.push(Type::String),
        //                 Some(ValueType::NONE) => self.types.push(Type::None),
        //                 Some(ValueType::FUNCTION(_, _)) => self.types.push(Type::Function),
        //                 a => {
        //                     return Err(Error::new(
        //                         common::error::ErrorOrigin::RUNTIME,
        //                         "Unknown type".to_string(),
        //                     ));
        //                 }
        //             }
        //         }
        //
        //         Operation::Add | Operation::Subtract | Operation::Multiply | Operation::Divide => {
        //             match (self.types.pop(), self.types.pop()) {
        //                 (Some(Type::Integer), Some(Type::Integer)) => {
        //                     self.types.push(Type::Integer);
        //                 }
        //                 (Some(Type::Float), Some(Type::Float)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::Integer), Some(Type::Float)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::Float), Some(Type::Integer)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::String), Some(Type::String)) => {
        //                     self.types.push(Type::String);
        //                 }
        //                 _ => todo!("Not implemented operation check"),
        //             }
        //         }
        //         Operation::Modulo
        //         | Operation::BitAnd
        //         | Operation::BitOr
        //         | Operation::BitXor
        //         | Operation::LeftShift
        //         | Operation::RightShift => {
        //             if let (Some(Type::Integer), Some(Type::Integer)) =
        //                 (self.types.pop(), self.types.pop())
        //             {
        //                 self.types.push(Type::Integer);
        //             } else {
        //                 todo!("Operation not supported for types other than integers")
        //             }
        //         }
        //         Operation::Equal
        //         | Operation::Less
        //         | Operation::LessEqual
        //         | Operation::Greater
        //         | Operation::GreaterEqual => {
        //             if let (Some(rhs), Some(lhs)) = (self.types.pop(), self.types.pop()) {
        //                 if lhs != rhs {
        //                     todo!("Unable to handle comparison between incompatible types")
        //                 } else {
        //                     self.types.push(Type::Bool)
        //                 }
        //             }
        //         }
        //         Operation::Argument => {
        //             // name, type, offset
        //             variables.insert(
        //                 op.get(0).cloned().unwrap_or(0),
        //                 (op.get(1).cloned().unwrap_or(0)).into(),
        //             );
        //         }
        //         Operation::Load => {
        //             self.types.push(
        //                 variables
        //                     .get(&op.operands()[0])
        //                     .cloned()
        //                     .unwrap_or_default(),
        //             );
        //         }
        //         _ => (),
        //     }
        // }
    }
}
