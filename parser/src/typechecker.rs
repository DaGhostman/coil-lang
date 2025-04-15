use common::{
    error::{Error, ErrorOrigin},
    opcodes::{IR, Operation},
    program::data::Data,
    types::{Kind, Type},
};

// use std::collections::HashMap;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug)]
pub struct TypeChecker<const N: usize> {
    stack: [Type; N],
    sp: usize,
    errors: Vec<Error>,
    classes: HashMap<usize, HashMap<usize, Type>>,
    functions: HashMap<usize, Type>,
}

impl<const N: usize> Default for TypeChecker<N> {
    fn default() -> Self {
        Self {
            sp: 0,
            stack: [Type::default(); N],
            errors: Vec::with_capacity(8),
            classes: HashMap::default(),
            functions: HashMap::default(),
        }
    }
}

macro_rules! binary_op {
    ($this:expr, $l:ident, $r: ident) => {
        if ($this.peek(0).kind() == Kind::$r && $this.peek(1).kind() == Kind::$l) {
            $this.pop(2);
            true
        } else {
            false
        }
    };
}

impl<const N: usize> TypeChecker<N> {
    fn push(&mut self, type_: Type) {
        self.stack[self.sp] = type_;
        self.sp += 1;
    }

    fn pop(&mut self, n: usize) -> Type {
        let sp = self.sp - 1;
        self.sp -= n;
        self.stack[sp]
    }

    fn peek(&self, offset: usize) -> Type {
        self.stack[self.sp - 1 - offset]
    }

    fn last(&self) -> Type {
        self.stack[self.sp - 1]
    }

    pub fn get_errors(&self) -> &Vec<Error> {
        &self.errors
    }

    fn error(&mut self, message: String, start: (usize, usize), end: (usize, usize)) {
        let mut fmt = format!("{message}");
        if start.0 != end.0 {
            fmt = format!("{fmt} @ {}:{}-{}:{}", start.0, start.1, end.0, end.1);
        } else {
            fmt = format!("{fmt} @ {}:{}-{}", start.0, start.1, end.1);
        }

        self.errors.push(Error::new(ErrorOrigin::PARSE, fmt));
    }

    fn clear(&mut self) {
        self.sp = 0;
        self.stack = [Type::new(Kind::None); N];
    }

    pub fn check(&mut self, code: &[IR], data: &mut Data) -> Vec<IR> {
        self.clear();
        let mut bytecode = Vec::with_capacity(code.len());
        let mut variables = HashMap::default();

        let mut ip = 0;

        while ip < code.len() {
            let mut op = code[ip];

            match op.code() {
                Operation::Const => {
                    self.push(*data.constant_type(op.operands()[0]));
                }
                Operation::Add
                | Operation::Subtract
                | Operation::Multiply
                | Operation::Modulo
                | Operation::Divide => {
                    if binary_op!(self, Integer, Integer) {
                        self.push(Type::new(Kind::Integer));
                    } else if binary_op!(self, Integer, Float)
                        || binary_op!(self, Float, Integer)
                        || binary_op!(self, Float, Float)
                    {
                        self.push(Type::new(Kind::Float));
                    } else {
                        self.errors.push(Error::new(
                            ErrorOrigin::PARSE,
                            "Invalid operation".to_string(),
                        ));
                    }
                }
                Operation::Store | Operation::Declare | Operation::Assign | Operation::Argument => {
                    let [name, ..] = op.operands();

                    if variables.contains_key(name) {
                        let ty = self.pop(1);
                        if ty != variables[name] {
                            self.errors.push(Error::new(
                                ErrorOrigin::COMPILE,
                                format!("Unable to assign value of type {:?} to '{}' because it expects {:?}", ty, data.symbol_name(*name), variables[name])
                            ));
                        }
                    } else {
                        variables.insert(op.operands()[0], op.kind());
                    }
                }
                Operation::Instantiate => {
                    self.push(op.kind());
                }
                Operation::Load => {
                    self.push(variables[&op.operands()[0]]);
                }
                Operation::Call => {
                    let [name, arity, _] = op.operands();
                    let mut kind = Type::new(Kind::None);
                    if let Some(func) = self.functions.get(name) {
                        for idx in 0..*arity {
                            if func.get(idx) != self.peek(idx).kind() {
                                self.errors.push(Error::new(ErrorOrigin::PARSE, format!("Argument #{} of function '{}' does not match expected type {:?}, got {:?}", idx + 1, data.symbol_name(op.operands()[0]), func.get(idx), self.peek(idx))));
                            }
                        }

                        kind = Type::new(func.returns());
                    }

                    self.pop(*arity);
                    self.push(kind);
                }
                Operation::This => {
                    self.push(op.kind());
                }
                Operation::Invoke => {
                    let [name, arity, _] = op.operands();
                    let mut result = Type::new(Kind::None);
                    if let Kind::Object(n) = self.peek(*arity).kind() {
                        let method = self.classes[&n][name];

                        for idx in 0..*arity {
                            if method.get(idx) != self.peek(idx).kind() {
                                self.errors.push(Error::new(ErrorOrigin::PARSE, format!("Argument #{} of method '{}::{}' does not match expected type {:?}, got {:?}", idx + 1, data.symbol_name(n), data.symbol_name(op.operands()[0]), method.get(idx), self.peek(idx))));
                            }
                        }

                        result = Type::new(method.returns());
                        op = IR::new(Operation::Invoke, Some([*name, *arity, n]));
                    }
                    self.pop(1 + op.operands()[1]);
                    self.push(result);

                    // dbg!(self.peek(op.operands()[1]), &self.stack[..self.sp]);
                    // self.pop(1 + op.operands()[1]);
                    // self.push(op.kind());
                    // dbg!(&self.stack[..self.sp]);
                    // dbg!(op.operands());
                }
                Operation::Function => {
                    self.functions.insert(op.operands()[0], op.kind());
                }
                Operation::Method => {
                    let [owner, name, _] = op.operands();

                    self.classes
                        .entry(*owner)
                        .and_modify(|entry| {
                            entry.insert(*name, op.kind());
                        })
                        .or_insert_with(|| {
                            let mut state = HashMap::default();
                            state.insert(*name, op.kind());

                            state
                        });
                }
                Operation::Greater
                | Operation::GreaterEqual
                | Operation::LessEqual
                | Operation::Less => {
                    if !binary_op!(self, Integer, Integer) && !binary_op!(self, Float, Float) {
                        self.errors.push(Error::new(
                            ErrorOrigin::PARSE,
                            "Invalid comparison".to_string(),
                        ));
                    } else {
                        self.push(Type::new(Kind::Bool));
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

// impl<const N: usize> CompilationPass for TypeChecker<N> {
//     fn compile(
//         &mut self,
//         code: &[common::opcodes::IR],
//         data: &mut Data,
//     ) -> Result<Vec<common::opcodes::IR>, Error> {
//         let code = self.do_compile(code, data);
//
//         if !self.errors.is_empty() {
//             let error = (Error::new(
//                 ErrorOrigin::COMPILE,
//                 "Unable to finish compilation due to the following type errors".to_string(),
//             ));
//
//             for error in &self.errors {
//                 eprintln!("{}", error);
//             }
//
//             return Err(error);
//         }
//
//         Ok(code)
//         // let mut variables: HashMap<usize, Type> = HashMap::new();
//         //
//         // for op in program.code() {
//         //     match op.code() {
//         //         Operation::Const => {
//         //             match program
//         //                 .constant(op.get(0).copied().unwrap_or_default())
//         //                 .map(|v| v.kind())
//         //             {
//         //                 Some(ValueType::BOOLEAN(_)) => self.types.push(Type::Bool),
//         //                 Some(ValueType::INTEGER(_)) => self.types.push(Type::Integer),
//         //                 Some(ValueType::FLOAT(_)) => self.types.push(Type::Float),
//         //                 Some(ValueType::STRING(_)) => self.types.push(Type::String),
//         //                 Some(ValueType::NONE) => self.types.push(Type::None),
//         //                 Some(ValueType::FUNCTION(_, _)) => self.types.push(Type::Function),
//         //                 a => {
//         //                     return Err(Error::new(
//         //                         common::error::ErrorOrigin::RUNTIME,
//         //                         "Unknown type".to_string(),
//         //                     ));
//         //                 }
//         //             }
//         //         }
//         //
//         //         Operation::Add | Operation::Subtract | Operation::Multiply | Operation::Divide => {
//         //             match (self.types.pop(), self.types.pop()) {
//         //                 (Some(Type::Integer), Some(Type::Integer)) => {
//         //                     self.types.push(Type::Integer);
//         //                 }
//         //                 (Some(Type::Float), Some(Type::Float)) => {
//         //                     self.types.push(Type::Float);
//         //                 }
//         //                 (Some(Type::Integer), Some(Type::Float)) => {
//         //                     self.types.push(Type::Float);
//         //                 }
//         //                 (Some(Type::Float), Some(Type::Integer)) => {
//         //                     self.types.push(Type::Float);
//         //                 }
//         //                 (Some(Type::String), Some(Type::String)) => {
//         //                     self.types.push(Type::String);
//         //                 }
//         //                 _ => todo!("Not implemented operation check"),
//         //             }
//         //         }
//         //         Operation::Modulo
//         //         | Operation::BitAnd
//         //         | Operation::BitOr
//         //         | Operation::BitXor
//         //         | Operation::LeftShift
//         //         | Operation::RightShift => {
//         //             if let (Some(Type::Integer), Some(Type::Integer)) =
//         //                 (self.types.pop(), self.types.pop())
//         //             {
//         //                 self.types.push(Type::Integer);
//         //             } else {
//         //                 todo!("Operation not supported for types other than integers")
//         //             }
//         //         }
//         //         Operation::Equal
//         //         | Operation::Less
//         //         | Operation::LessEqual
//         //         | Operation::Greater
//         //         | Operation::GreaterEqual => {
//         //             if let (Some(rhs), Some(lhs)) = (self.types.pop(), self.types.pop()) {
//         //                 if lhs != rhs {
//         //                     todo!("Unable to handle comparison between incompatible types")
//         //                 } else {
//         //                     self.types.push(Type::Bool)
//         //                 }
//         //             }
//         //         }
//         //         Operation::Argument => {
//         //             // name, type, offset
//         //             variables.insert(
//         //                 op.get(0).cloned().unwrap_or(0),
//         //                 (op.get(1).cloned().unwrap_or(0)).into(),
//         //             );
//         //         }
//         //         Operation::Load => {
//         //             self.types.push(
//         //                 variables
//         //                     .get(&op.operands()[0])
//         //                     .cloned()
//         //                     .unwrap_or_default(),
//         //             );
//         //         }
//         //         _ => (),
//         //     }
//         // }
//     }
// }
