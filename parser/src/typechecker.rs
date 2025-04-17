use common::{
    error::{Error, ErrorOrigin},
    opcodes::{IR, Metadata, Operation},
    program::data::Data,
    types::{Kind, Type},
};

use rustc_hash::FxHashMap as HashMap;

pub struct TypeChecker<const N: usize> {
    file: String,
    stack: [usize; N],
    sp: usize,
    scope: usize,
    errors: Vec<Error>,
    classes: HashMap<usize, HashMap<usize, (usize, usize, bool)>>,
    class_params: HashMap<usize, HashMap<usize, usize>>,
    state: HashMap<usize, HashMap<usize, usize>>,
    functions: HashMap<usize, usize>,

    type_arguments: HashMap<usize, HashMap<usize, usize>>,
    expects_return: Option<usize>,
}

impl<const N: usize> Default for TypeChecker<N> {
    fn default() -> Self {
        Self {
            file: String::new(),
            sp: 0,
            scope: 0,
            stack: [0; N],
            errors: Vec::with_capacity(8),
            classes: HashMap::default(),
            class_params: HashMap::default(),
            state: HashMap::default(),
            functions: HashMap::default(),
            // ---
            type_arguments: HashMap::default(),
            expects_return: None,
        }
    }
}

macro_rules! binary_op {
    ($this:expr, $l:ident, $r: ident) => {
        if ($this.peek(0) == Kind::$r && $this.peek(1) == Kind::$l) {
            $this.pop(2);
            true
        } else {
            false
        }
    };
}

impl<const N: usize> TypeChecker<N> {
    fn push(&mut self, r#type: usize) {
        self.stack[self.sp] = r#type;
        self.sp += 1;
    }

    fn pop(&mut self, n: usize) -> usize {
        if n <= 0 {
            return 0;
        }

        let sp = self.sp - 1;
        self.sp -= n;
        self.stack[sp]
    }

    fn peek(&self, offset: usize) -> usize {
        self.stack[self.sp - 1 - offset]
    }

    fn last(&self) -> usize {
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
            format!("'{message}' in {}@{metadata}", self.file),
        ));
    }

    fn clear(&mut self) {
        self.sp = 0;
        self.stack = [0; N];
    }

    fn resolve_type(&self, data: &Data, entry: usize) -> usize {
        match data.get_type(entry).kind() {
            Kind::Generic(name, constraint) => {
                if self.type_arguments.contains_key(&self.scope) && self.type_arguments[&self.scope].contains_key(&name) {
                    self.resolve_type(data, self.type_arguments[&self.scope][&name])
                } else {
                    entry
                }
            }
            _ => entry,
        }
    }

    fn match_type(&self, data: &Data, actual: usize, expectation: usize, ) -> bool {
        let ty = data.get_type(self.resolve_type(data, actual));
        let expectation = self.resolve_type(data, expectation);

        match ty.kind() {
            Kind::Union => {
                for idx in 0..ty.len() {
                    if self.match_type(data, ty.get(idx), expectation) {
                        return true
                    }
                }

                false
            }
            Kind::Intersection => {
                let mut state = false;
                for idx in 0..ty.len() {
                    if !state { break; }
                    state = self.match_type(data, ty.get(idx), expectation);
                }

                state
            }
            Kind::Generic(_, constraint) => {
                expectation == actual || self.match_type(data, actual, constraint)
            }
            _ => {
                let a = ty;
                let e = data.get_type(expectation);

                if a.kind() == e.kind() {
                    if a.len() != e.len() {
                        return false;
                    }

                    for idx in 0..a.len(){
                        if !self.match_type(data, a.get(idx), e.get(idx)) {
                            return false;
                        }
                    }

                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn check(&mut self, code: &[IR], data: &Data) -> Vec<IR> {
        let sp = self.sp;
        self.clear();
        let mut bytecode = Vec::with_capacity(code.len());
        let mut variables = HashMap::default();

        let mut ip = 0;

        while ip < code.len() {
            let op = &code[ip];

            // println!("#{: >08}\t{: >12} [{}]", ip, format!("{:?}", &code[ip].code()), self.stack[..self.sp].iter().map(|n| {
            //     data.get_type(*n).output(data)
            // }).collect::<Vec<String>>().join(", "));

            match op.code() {
                Operation::Const => {
                    self.push(data.find_type(*data.constant_type(op.operands()[0])));
                }
                Operation::ClassParam => {
                    let [owner, name, ..] = op.operands();


                    self.class_params.entry(*owner).and_modify(|entry| {
                        entry.insert(*name, op.kind());
                    }).or_insert_with(|| {
                        let mut params = HashMap::default();
                        params.insert(*name, op.kind());

                        params
                    });
                }
                Operation::Add
                | Operation::Subtract
                | Operation::Multiply
                | Operation::Modulo
                | Operation::Divide => {
                    let rhs = self.pop(1);
                    let lhs = self.pop(1);

                    let int = data.find_type(Type::integer());
                    let float = data.find_type(Type::float());

                    if self.match_type(data, rhs, int) && self.match_type(data, lhs, int) {
                        self.push(int)
                    } else if 
                        (self.match_type(data, rhs, int) && self.match_type(data, lhs, int)) ||
                        (self.match_type(data, rhs, float) && self.match_type(data, lhs, float)) ||
                        (self.match_type(data, rhs, int) && self.match_type(data, lhs, float)) ||
                        (self.match_type(data, rhs, float) && self.match_type(data, lhs, int))
                    {
                        self.push(float);
                    } else {
                        self.error(
                            format!(
                                "Unsupported operation '{:?}' using, {:?} and {:?}",
                                op.code(),
                                data.get_type(lhs).output(data),
                                data.get_type(rhs).output(data),
                            ),
                            op.metadata().unwrap(),
                        );
                    }
                }
                Operation::Greater
                | Operation::GreaterEqual
                | Operation::LessEqual
                | Operation::Less => {
                    let rhs = self.pop(1);
                    let lhs = self.pop(1);

                    let int = data.find_type(Type::integer());
                    let float = data.find_type(Type::float());
                    if !(self.match_type(data, lhs, int) && self.match_type(data, rhs, int)) 
                        && !(self.match_type(data, lhs, float) && self.match_type(data, rhs, float)) 
                    {
                        self.errors.push(Error::new(
                            ErrorOrigin::PARSE,
                            "Invalid comparison".to_string(),
                        ));
                    } 

                    self.push(data.find_type(Type::bool()));
                }
                Operation::Equal | Operation::NotEqual => {
                    self.pop(2);

                    self.push(data.find_type(Type::bool()));
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
                                    format!(
                                        "Unable to assign value of type {:?} to '{}' because it expects {:?}", 
                                        ty,
                                        data.symbol_name(*name),
                                        data.get_type(variables[name]).output(data)
                                    ),
                                    op.metadata().unwrap(),

                                );
                            }
                        } else {
                            variables.insert(
                                name,
                                if op.kind() != data.find_type(Type::void()) {
                                    op.kind()
                                } else {
                                    ty
                                },
                            );
                        }
                    } else {
                        variables.insert(
                            name,
                            if op.kind() != data.find_type(Type::void()) {
                                op.kind()
                            } else {
                                ty
                            },
                        );
                    }
                }
                Operation::Instantiate => {
                    let instance = data.get_type(op.kind());
                    if let Kind::Object(n) = instance.kind() {
                        // let definition = self.class_params[&n];
                        for param in instance.arguments() {
                            if let Kind::Generic(name, substitute) = data.get_type(*param).kind() {
                                if !self.class_params[&n].contains_key(&name) {
                                    self.error(format!(
                                        "Unknown generic parameter: ${} used for {}",
                                        data.symbol_name(name),
                                        data.symbol_name(n),
                                    ), op.metadata().unwrap());
                                    continue;
                                }

                                let constraint = self.resolve_type(data, self.class_params[&n][&name]);
                                if !self.match_type(data, substitute, constraint) && constraint != 0 {
                                    self.error(format!(
                                        "Generic parameter ${} = '{}' is constrained by '{}', which has not been satisfied",
                                        data.symbol_name(name),
                                        data.get_type(substitute).output(data),
                                        data.get_type(constraint).output(data),
                                    ), op.metadata().unwrap());
                                }
                            }
                        }


                        self.push(op.kind());
                    }
                }, 
                Operation::This => self.push(op.kind()),
                Operation::Load => self.push(variables[&op.operands()[0]]),
                Operation::Call => {
                    let [name, arity, _] = op.operands();
                    let mut kind = data.find_type(Type::void());
                    if let Some(func) = self.functions.get(name) {
                        for idx in 0..*arity {
                            if !self.match_type(data, data.get_type(*func).get(idx), self.peek(idx)) {
                                self.errors.push(Error::new(ErrorOrigin::PARSE, format!(
                                    "Argument #{} of function '{}' does not match expected type {:?}, got {:?}",
                                    idx + 1,
                                    data.symbol_name(op.operands()[0]),
                                    data.get_type(data.get_type(*func).get(idx)).output(data), 
                                    data.get_type(self.peek(idx)).output(data)
                                )));
                            }
                        }

                        kind = self.resolve_type(data, data.get_type(*func).returns());
                    }

                    self.pop(*arity);
                    self.push(kind);
                }
                Operation::Prop => {
                    let [name, action, ..] = op.operands();
                    match action {
                        0 => {
                            if let Kind::Object(n) = data.get_type(self.peek(0)).kind() {
                                self.push(self.state[&n][name]);
                            }
                        }
                        1 => {
                            if let Kind::Object(n) = data.get_type(self.peek(1)).kind() {
                                let kind = self.peek(0);
                                self.state
                                    .entry(n)
                                    .and_modify(|entry| {
                                        entry.insert(*name, kind);
                                    })
                                    .or_insert_with(|| {
                                        let mut props = HashMap::default();
                                        props.insert(*name, kind);
                                        props
                                    });
                                self.pop(2);
                            }
                        }
                        2 => {
                            let owner = (name & 0xFFFFFFFF00000000) >> 32;
                            let name = name & 0x00000000FFFFFFFF;

                            self.state
                                .entry(owner)
                                .and_modify(|entry| {
                                    entry.insert(name, op.kind());
                                })
                                .or_insert_with(|| {
                                    let mut props = HashMap::default();
                                    props.insert(name, op.kind());
                                    props
                                });
                            ip += 1;
                            continue;
                        }
                        _ => {
                            self.error(
                                "Unable to perform unknown operation on property".to_string(),
                                op.metadata().unwrap(),
                            );
                        }
                    }
                }
                Operation::Invoke => {
                    let [name, call_arity, _] = op.operands();
                    let mut result = data.find_type(Type::void());
                    let object = data.get_type(self.peek(*call_arity));

                    let scope = self.scope;
                    self.scope = self.peek(*call_arity);
                    for param in object.arguments() {
                        if let Kind::Generic(n, t) = data.get_type(*param).kind() {
                            self.type_arguments.entry(self.peek(*call_arity))
                                .and_modify(|entry| {
                                    entry.insert(n, t);
                                }).or_insert_with(|| {
                                    let mut row = HashMap::default();
                                    row.insert(n, t);

                                    row

                                });
                        }
                    }

                    if let Kind::Object(n) = object.kind() {
                        let (method, declared_arity, public) = self.classes[&n][name];
                        let existing_method = data.get_type(method);

                        let fqn = format!("{}::{}", data.symbol_name(n), data.symbol_name(*name));

                        if *call_arity != declared_arity {
                            self.error(format!("Called '{fqn}' with {call_arity} argument(s), but it was defined with {declared_arity}"), op.metadata().unwrap());
                        }

                        for idx in 0..*call_arity {
                            if !self.match_type(data, existing_method.get(idx), self.peek(idx)) {
                                self.errors.push(Error::new(ErrorOrigin::PARSE, format!("Argument #{} of method '{fqn}' does not match expected type {:?}, got {:?}", idx + 1, data.get_type(self.resolve_type(data, existing_method.get(idx))).output(data), data.get_type(self.resolve_type(data, self.peek(idx))).output(data))));
                            }
                        }

                        if !public {
                            if self.scope != n {
                                self.error(
                                    format!(
                                        "Calling a private method '{fqn}' from outside is forbidden"
                                    ),
                                    op.metadata().unwrap(),
                                );
                            }
                        }

                        result = self.resolve_type(data, data.get_type(method).returns());
                        bytecode.push(IR::new(Operation::Invoke, Some([*name, *call_arity, n])));
                    }
                    self.pop(1 + call_arity);
                    self.push(result);
                    ip += 1;

                    self.scope = scope;
                    continue;
                }
                Operation::Function => {
                    let [name, _, len] = op.operands();

                    self.functions.insert(*name, op.kind());

                    bytecode.push(*op);
                    self.expects_return = Some(self.resolve_type(data, data.get_type(op.kind()).returns()));
                    bytecode.append(&mut self.check(&code[ip + 1..ip + 1 + len], data));

                    ip += len + 1;
                    continue;
                }
                Operation::Method => {
                    let &[mut symbol, arity, len] = op.operands();

                    let public = (symbol & 1) == 1;
                    symbol >>= 1;
                    let name = symbol & 0xffff;
                    symbol >>= 16;
                    let owner = symbol;

                    self.classes
                        .entry(owner)
                        .and_modify(|entry| {
                            entry.insert(name, (op.kind(), arity, public));
                        })
                        .or_insert_with(|| {
                            let mut state = HashMap::default();
                            state.insert(name, (op.kind(), arity, public));

                            state
                        });

                    let scope = self.scope;
                    self.scope = owner;
                    bytecode.push(*op);
                    self.expects_return = Some(data.get_type(op.kind()).returns());
                    bytecode.append(&mut self.check(&code[ip + 1..ip + 1 + len], data));
                    self.expects_return = None;
                    self.scope = scope;
                    ip += len + 1;
                    continue;
                }
                Operation::Leave => {
                    if let Some(expected) = self.expects_return {
                        let expected_ty = data.get_type(expected);
                        dbg!(expected_ty.kind());
                        if !self.match_type(data, self.peek(0), expected) && (self.peek(0) != 0) {
                            self.error(format!(
                                "Expected to return '{}' but it has branch that returns '{}'",
                                data.get_type(self.resolve_type(data, expected)).output(data),
                                data.get_type(self.resolve_type(data, self.peek(0))).output(data),
                            ), op.metadata().unwrap());
                        }
                    }
                }
                _ => (),
            }

            bytecode.push(*op);
            ip += 1;
        }

        self.sp = sp;
        bytecode
    }
}
