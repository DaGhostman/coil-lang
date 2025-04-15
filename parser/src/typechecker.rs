use common::{
    error::{Error, ErrorOrigin},
    opcodes::{IR, Metadata, Operation},
    program::data::Data,
    types::{Kind, Type},
};

use rustc_hash::FxHashMap as HashMap;

pub struct TypeChecker<const N: usize> {
    file: String,
    stack: [Type; N],
    sp: usize,
    errors: Vec<Error>,
    classes: HashMap<usize, HashMap<usize, Type>>,
    functions: HashMap<usize, Type>,
}

impl<const N: usize> Default for TypeChecker<N> {
    fn default() -> Self {
        Self {
            file: String::new(),
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

    pub fn set_file(&mut self, file: String) {
        self.file = file;
    }

    fn error(&mut self, message: String, metadata: &Metadata) {
        // if start.0 != end.0 {
        //     fmt = format!("{fmt} @ {}:{}-{}:{}", start.0, start.1, end.0, end.1);
        // } else {
        //     fmt = format!("{fmt} @ {}:{}-{}", start.0, start.1, end.1);
        // }

        self.errors.push(Error::new(
            ErrorOrigin::TYPE,
            format!("'{message}' in {metadata}"),
        ));
    }

    fn clear(&mut self) {
        self.sp = 0;
        self.stack = [Type::new(Kind::None); N];
    }

    pub fn check(&mut self, code: &[IR], data: &Data) -> Vec<IR> {
        self.clear();
        let mut bytecode = Vec::with_capacity(code.len());
        let mut variables = HashMap::default();

        let mut ip = 0;

        while ip < code.len() {
            let mut op = &code[ip];

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
                        self.error(
                            format!(
                                "Unsupported operation '{:?}' using, {:?} and {:?}",
                                op.code(),
                                self.peek(1),
                                self.peek(0)
                            ),
                            op.metadata().unwrap(),
                        );
                    }
                }
                Operation::Argument => {
                    let [name, ..] = op.operands();
                    variables.insert(name, op.kind());
                }
                Operation::Store | Operation::Declare | Operation::Assign => {
                    let [name, ..] = op.operands();

                    let ty = self.pop(1);
                    if variables.contains_key(name) {
                        if variables.contains_key(name) {
                            if ty != variables[name]
                            /* && variables[name].kind() != Kind::None */
                            {
                                self.error(
                                    format!("Unable to assign value of type {:?} to '{}' because it expects {:?}", ty, data.symbol_name(*name), variables[name]),
                                    op.metadata().unwrap(),

                                );
                            }
                        } else {
                            variables.insert(
                                name,
                                if op.kind().kind() != Kind::None {
                                    op.kind()
                                } else {
                                    ty
                                },
                            );
                        }
                    } else {
                        variables.insert(
                            name,
                            if op.kind().kind() != Kind::None {
                                op.kind()
                            } else {
                                ty
                            },
                        );
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
                    }
                    self.pop(1 + arity);
                    self.push(result);
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

            bytecode.push(*op);
            ip += 1;
        }

        bytecode
    }
}
