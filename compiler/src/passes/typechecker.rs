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
        if self.sp >= N {
            self.errors.push(Error::new(
                ErrorOrigin::COMPILE,
                "Type stack overflow".to_string(),
            ));
            return;
        }
        self.stack[self.sp] = type_;
        self.sp += 1;
    }

    fn pop(&mut self, n: usize) -> Type {
        let sp = self.sp;
        self.sp -= n;
        self.stack[sp - n]
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
                    let right = self.pop(1);
                    let left = self.pop(1);
                    let type_ = match (left.kind(), right.kind()) {
                        (Kind::Integer, Kind::Integer) => Kind::Integer,
                        (Kind::Float, Kind::Float) => Kind::Float,
                        (Kind::Integer, Kind::Float) => Kind::Float,
                        (Kind::Float, Kind::Integer) => Kind::Float,
                        (r, l) => {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!(
                                    "{:?} {:?} {:?} has invalid types and is therefore not allowed",
                                    l, op.byte(), r
                                ),
                            ));
                            Kind::None
                        }
                    };

                    self.push(Type::new(type_));
                }
                Byte::Less | Byte::LessEqual | Byte::Equal | Byte::Greater | Byte::GreaterEqual => {
                    let right = self.pop(1);
                    let left = self.pop(1);

                    // Only allow comparisons between compatible types
                    match (left.kind(), right.kind()) {
                        (Kind::Integer, Kind::Integer) => (),
                        (Kind::Float, Kind::Float) => (),
                        (Kind::String, Kind::String) => (),
                        (Kind::Object(_), Kind::Object(_)) => (),
                        (Kind::List(_), Kind::List(_)) => (),
                        _ => {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!("Unable to compare incompatible types: {:?} and {:?}", left, right),
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
                    if !variables.contains_key(&op.operand(0)) {
                        self.errors.push(Error::new(
                            ErrorOrigin::COMPILE,
                            format!("Undefined variable: {}", op.operand(0)),
                        ));
                    }
                    self.push(
                        variables
                            .get(&op.operand(0))
                            .copied()
                            .unwrap_or_else(|| op.get_type()),
                    );
                }
                Byte::Instantiate => {
                    self.push(op.get_type());
                }
                Byte::Invoke => {
                    if !matches!(self.peek(op.operand(1)).kind(), Kind::Object(_)) {
                        self.errors.push(Error::new(
                            ErrorOrigin::COMPILE,
                            "Unable to invoke a method on non-object".to_string(),
                        ));
                    }
                    // Check method existence and argument types
                    let obj_type = self.peek(op.operand(1));
                    if let Kind::Object(methods) = obj_type.kind() {
                        let method_name = op.operand(0);
                        if !methods.contains_key(&method_name) {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!("Method '{}' does not exist on object", method_name),
                            ));
                        } else {
                            let method_type = methods.get(&method_name).unwrap();
                            // TODO: Check argument types against method signature
                        }
                    }
                }
                _ => (),
            }

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
    }
}