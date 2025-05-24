use common::{
    error::{Message, MessageOrigin},
    opcodes::{Metadata, Operation, IR},
    program::data::Data,
    types::{Kind, Type}
};

use rustc_hash::FxHashMap as HashMap;

pub struct TypeChecker<const N: usize> {
    file: String,
    stack: [usize; N],
    sp: usize,
    scope: usize,
    messages: Vec<Message>,
    classes: HashMap<usize, HashMap<usize, (usize, usize, bool)>>,
    class_params: HashMap<usize, HashMap<usize, usize>>,
    state: HashMap<usize, HashMap<usize, usize>>,
    functions: HashMap<usize, Type>,
    aliases: HashMap<usize, usize>,

    type_arguments: HashMap<usize, HashMap<usize, usize>>,
    expects_return: Option<usize>,
}

impl <const N: usize> TypeChecker<N> {
    pub fn register(&mut self, name: usize, ty_: Type) {
        self.functions.insert(name, ty_);
    }

    pub fn alias(&mut self, name: usize, fqn: usize) {
        self.aliases.insert(name, fqn);
    }

    pub fn resolve(&self, name: usize) -> usize {
        if let Some(name) = self.aliases.get(&name) {
            *name
        } else {
            name
        }
    }

    pub(crate) fn aliases(&self) -> &HashMap<usize, usize> {
        &self.aliases
    }

    pub(crate) fn functions(&self) -> &HashMap<usize, Type> {
        &self.functions
    }

    pub(crate) fn restore(&mut self, functions: HashMap<usize, Type>, aliases: HashMap<usize, usize>) {
        self.functions = functions;
        self.aliases = aliases;
    }
}

impl<const N: usize> Default for TypeChecker<N> {
    fn default() -> Self {
        Self {
            file: String::new(),
            sp: 0,
            scope: 0,
            stack: [0; N],
            messages: Vec::with_capacity(8),
            classes: HashMap::default(),
            class_params: HashMap::default(),
            state: HashMap::default(),
            functions: HashMap::default(),
            aliases: HashMap::default(),
            // ---
            type_arguments: HashMap::default(),
            expects_return: None,
        }
    }

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


    pub fn get_messages(&self) -> &Vec<Message> {
        &self.messages
    }

    pub fn set_file(&mut self, file: String) {
        self.file = file;
    }

    // fn warning(&mut self, message: String, metadata: &Metadata) {
    //     self.messages.push(Message::warning(
    //         MessageOrigin::TYPE,
    //         format!("'{message}' in {}@{metadata}", self.file),
    //     ));
    // }

    fn error(&mut self, message: String, metadata: &Metadata) {
        let mut msg = Message::error(
            MessageOrigin::TYPE,
            format!("{message}"),
        );

        msg.set_location(format!("{}@{metadata}", self.file));
        self.messages.push(msg)
    }
    
    fn warn(&mut self, message: String, metadata: &Metadata) {
        let mut msg = Message::warning(
            MessageOrigin::TYPE,
            format!("{message}"),
        );

        msg.set_location(format!("{}@{metadata}", self.file));
        self.messages.push(msg)
    }

    fn clear(&mut self) {
        self.sp = 0;
        self.stack = [0; N];
    }

    fn resolve_type(&self, data: &Data, entry: usize) -> usize {
        match data.get_type(entry).kind() {
            Kind::Generic(name, constraint) => {
                if self.type_arguments.contains_key(&self.scope) && self.type_arguments[&self.scope].contains_key(&name) {
                    data.find_type(Type::new(Kind::Generic(name, self.type_arguments[&self.scope][&name])))
                } else if self.class_params.contains_key(&self.scope) && self.class_params[&self.scope].contains_key(&name) {
                    data.find_type(Type::new(Kind::Generic(name, self.class_params[&self.scope][&name])))
                } else {
                    constraint
                }
            }
            Kind::Coroutine(constraint) => {
                data.find_type(Type::new(Kind::Coroutine(self.resolve_type(data, constraint))))
            }
            _ => entry,
        }
    }

    fn substitute_type(&self, data: &mut Data, entry: usize) -> usize {

        match data.get_type(entry).kind() {
            Kind::Generic(name, constraint) => {
                if self.type_arguments.contains_key(&self.scope) && self.type_arguments[&self.scope].contains_key(&name) {
                
                    data.add_type(Type::new(Kind::Generic(name, self.type_arguments[&self.scope][&name])))
                } else if self.class_params.contains_key(&self.scope) && self.class_params[&self.scope].contains_key(&name) {
                    data.add_type(Type::new(Kind::Generic(name, self.class_params[&self.scope][&name])))
                } else {
                    constraint
                }
            }
            Kind::Coroutine(entry) => {
                data.add_type(Type::new(Kind::Coroutine(self.resolve_type(data, entry))))
            }
            _ => entry,
        }
    }

    fn match_type(&self, data: &Data, actual: usize, expectation: usize, ) -> bool {
        if expectation == 0 || actual == 0 || expectation == actual {
            return true;
        }

        let ty = data.get_type(self.resolve_type(data, expectation));
        let expectation = self.resolve_type(data, actual); 
        

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
                expectation == actual || actual == constraint || self.match_type(data, actual, constraint)
            }
            Kind::Coroutine(constraint) => {
                if let Kind::Coroutine(exp) = data.get_type(self.resolve_type(data, expectation)).kind() {
                    self.match_type(data, exp, constraint)
                } else {
                    false
                }
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

    pub fn check(&mut self, code: &[IR], data: &mut Data) -> Vec<IR> {
        let sp = self.sp;
        self.clear();
        let mut bytecode = Vec::with_capacity(code.len());
        let mut variables = HashMap::default();

        let mut ip = 0;

        while ip < code.len() {
            let op = &code[ip];

            #[cfg(feature = "debug")]
            println!("#{: >08}\t{: >12} [{}] - {}:{}", ip, format!("{:?}", &code[ip].code()), self.stack[..self.sp].iter().map(|n| {
                data.get_type(*n).output(data)
            }).collect::<Vec<String>>().join(", "),
                self.file,
                op.metadata(),
            );

            match op.code() {
                Operation::Const => {
                    self.push(data.find_type(*data.constant_type(op.operands()[0])));
                }
                Operation::Pop => {
                    self.pop(op.get(0));
                }
                Operation::Class => {
                    let [owner, len, ..] = op.operands();
                    let entry = self.class_params.entry(*owner).or_insert_with(|| {
                        HashMap::default()
                    });

                    let class = data.get_type(op.kind());
                    for param in class.arguments() {
                        if let Kind::Generic(n, _) = data.get_type(*param).kind() {
                            entry.insert(n, *param);
                        }
                    }

                    let mut body = self.check(&code[ip + 1..ip + len], data);
                    bytecode.push(IR::new(Operation::Class, [*owner, body.len(), 0]));
                    bytecode.append(&mut body);
                    ip += len;


                    continue;
                }
                Operation::Inc | Operation::Dec => {
                    self.push(variables[&op.get(0)]);
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
                    let string = data.find_type(Type::string());

                    if self.match_type(data, rhs, int) && self.match_type(data, lhs, int) {
                        self.push(int);
                    } else if 
                        (self.match_type(data, rhs, int) && self.match_type(data, lhs, int)) ||
                        (self.match_type(data, rhs, float) && self.match_type(data, lhs, float)) ||
                        (self.match_type(data, rhs, int) && self.match_type(data, lhs, float)) ||
                        (self.match_type(data, rhs, float) && self.match_type(data, lhs, int))
                    {
                        self.push(float);
                    } else if self.match_type(data, lhs, string) {
                        self.push(string);
                    } else {
                        self.error(
                            format!(
                                "Unsupported operation '{:?}' using, {:?} and {:?}",
                                op.code(),
                                data.get_type(lhs).output(data),
                                data.get_type(rhs).output(data),
                            ),
                            op.metadata(),
                        );
                    }
                }
                Operation::Pow => {
                    let rhs = self.pop(1);
                    let lhs = self.pop(1);

                    let int = data.add_type(Type::integer());
                    let float = data.add_type(Type::float());

                    if !(self.match_type(data, lhs, int) || self.match_type(data, lhs,float)) {
                        self.error(format!("Unable to raise value of type {lhs} to power"), op.metadata());
                    } else if !(self.match_type(data, rhs, int) || self.match_type(data, rhs, float)) {
                        self.error(format!("Unabel to use {rhs} as power"), op.metadata());
                    } else {
                        self.push(lhs);
                    }
                }
                Operation::LeftShift | Operation::RightShift | Operation::BitAnd | Operation::BitOr | Operation::BitXor => {
                    let rhs = self.pop(1);
                    let lhs = self.pop(1);

                    let int = data.add_type(Type::integer());
                    if !self.match_type(data, lhs, int) {
                        self.error(format!("Unable to perform bitwise operation on non-integer({lhs}) value"), op.metadata());
                    } else if !self.match_type(data, rhs, int) {
                        self.error(format!("Unabele to perform bitwise operation on integer using non-integer({rhs}) operand"), op.metadata());
                    }

                    self.push(int);
                },
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
                        self.error(
                            "Invalid comparison".to_string(),
                            op.metadata()
                        );
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
                Operation::Print => {
                    self.pop(1);
                }
                Operation::TypeOf => {
                    let _ = self.pop(1);

                    let constant_type = data.add_type(Type::new(Kind::Type));
                    self.push(constant_type);
                }
                Operation::Store | Operation::Declare | Operation::Assign => {
                    let [name, ..] = op.operands();

                    let ty = self.pop(1);

                        if let std::collections::hash_map::Entry::Vacant(e) = variables.entry(name) {
                            e.insert(if op.kind() == data.find_type(Type::any()) {
                                    ty
                                } else {
                                    op.kind()
                                });
                        } else if !self.match_type(data, ty, variables[&name]) {
                            self.error(
                                format!(
                                    "Unable to assign value of type {:?} to '{}' because it expects {:?}", 
                                    data.get_type(ty).output(data),
                                    data.symbol_name(*name),
                                    data.get_type(variables[&name]).output(data)
                                ),
                                op.metadata(),

                            );
                        }
                }
                Operation::Instantiate => {
                    let instance = data.get_type(op.kind());
                    if let Kind::Object(n) = instance.kind() {
                        if !self.classes.contains_key(&n) {
                            self.error(
                                format!("Attempting to instantiate a non-existing class '{}'", data.symbol_name(n)),
                                op.metadata(),
                            );
                        }

                        self.pop(op.get(1));


                        for param in instance.arguments() {
                            if let Kind::Generic(name, substitute) = data.get_type(*param).kind() {
                                if !self.class_params[&n].contains_key(&name) {
                                    self.error(format!(
                                        "Unknown generic parameter: ${} used for {}",
                                        data.symbol_name(name),
                                        data.symbol_name(n),
                                    ), op.metadata());
                                    continue;
                                }

                                let constraint = self.resolve_type(data, self.class_params[&n][&name]);
                                if !self.match_type(data, substitute, constraint) {
                                    self.error(format!(
                                        "Generic parameter ${} = '{}' is constrained by '{}', which has not been satisfied",
                                        data.symbol_name(name),
                                        data.get_type(substitute).output(data),
                                        data.get_type(constraint).output(data),
                                    ), op.metadata());
                                }
                            } 
                        }


                        self.push(op.kind());
                    }
                }, 
                Operation::This => self.push(op.kind()),
                Operation::Load => {
                    if variables.contains_key(&op.operands()[0]) {
                        self.push(variables[&op.operands()[0]]);
                    } else {
                        self.error(format!("Attempting to access undeclared variable '{}'", data.symbol_name(op.operands()[0])), op.metadata());
                        self.push(op.kind());
                    }
                }
                Operation::Range => {
                    let end = self.pop(1);
                    let start = self.pop(1);

                    let int = data.add_type(Type::integer());

                    if start == end && start == int {
                        let ty = data.add_type(Type::new(Kind::Range(start)));
                        self.push(ty);
                    } else {
                        self.error(format!("Unable to build a range with {start} and {end}"), op.metadata());
                    }
                }
                Operation::Iterate => {
                    let [name, ..] = op.operands();
                    if let Kind::Range(n) = data.get_type(self.peek(0)).kind() {
                        
                        variables.insert(name, n);
                    } else {
                        variables.insert(name, data.find_type(Type::any()));
                    }
                }
                Operation::Call => {
                    let &[name, arity, _] = op.operands();


                    let kind;

                    if let Some(&func) = self.functions.get(&self.resolve(name)) {
                        let fn_type = func;

                        for idx in 0..arity {
                            let sub = self.substitute_type(data, fn_type.get(idx));
                            if !self.match_type(data, self.peek(idx), sub) {
                                self.error(format!(
                                    "Argument #{} of function '{}' does not match expected type {:?}, got {:?}",
                                    arity - idx ,
                                    data.symbol_name(op.operands()[0]),
                                    data.get_type(func.get(idx)).output(data), 
                                    data.get_type(self.peek(idx)).output(data)
                                ), op.metadata());
                            }
                        }


                        kind = self.resolve_type(data, func.returns());
                    } else {
                        let fn_type = *data.get_type(op.kind());
                        let func = data.find_type(fn_type);

                        for idx in 0..arity {
                            let sub = self.substitute_type(data, fn_type.get(idx));
                            if !self.match_type(data, self.peek(idx), sub) {
                                self.error(format!(
                                    "Argument #{} of function '{}' does not match expected type {:?}, got {:?}",
                                    arity - idx ,
                                    data.symbol_name(op.operands()[0]),
                                    data.get_type(self.resolve_type(data, data.get_type(func).get(idx))).output(data), 
                                    data.get_type(self.resolve_type(data, self.peek(idx))).output(data)
                                ), op.metadata());
                            }
                        }

                        kind = self.resolve_type(data, fn_type.returns());
                    }

                    self.pop(arity);
                    self.push(kind);
                }
                Operation::Prop => {
                    let [name, action, ..] = op.operands();
                    match action {
                        0 => {
                            if let Kind::Object(n) = data.get_type(self.peek(0)).kind() {
                                self.pop(1);
                                if self.state[&n].contains_key(name) {
                                    self.push(self.state[&n][name]);
                                } else {
                                    self.error(format!("Attempting to access undeclared property '{}' on {}", data.symbol_name(*name), data.symbol_name(n)), op.metadata());
                                }
                            } else {
                                let ty =data.get_type(self.pop(1)).output(data); 
                                self.error(format!(
                                    "Unable to access property {} on {} as it is not an object",
                                    data.symbol_name(*name),
                                    ty,
                                ), op.metadata());
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
                                op.metadata(),
                            );
                        }
                    }
                }
                Operation::Invoke => {
                    let [name, arity, ..] = op.operands();
                    let mut result = data.find_type(Type::void());
                    let object = data.get_type(self.peek(*arity));

                    // let scope = self.scope;
                    // self.scope = self.peek(*arity);
                    for param in object.arguments() {
                        if let Kind::Generic(n, t) = data.get_type(*param).kind() {
                            self.type_arguments.entry(self.peek(*arity))
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

                        if *arity != declared_arity {
                            self.error(format!("Called '{fqn}' with {arity} argument(s), but it was defined with {declared_arity}"), op.metadata());
                        }

                        for idx in 0..*arity {
                            if !self.match_type(data, self.peek(idx), existing_method.get(idx), ) {
                                self.error(
                                    format!(
                                        "Argument #{} of method '{fqn}' does not match expected type {:?}, got {:?}",
                                        idx + 1,
                                        data.get_type(self.resolve_type(data, existing_method.get(idx))).output(data),
                                        data.get_type(self.resolve_type(data, self.peek(idx))).output(data)),
                                op.metadata());
                            }
                        }

                        if !public && self.scope != n {
                            self.error(
                                format!(
                                    "Calling a private method '{fqn}' from outside is forbidden"
                                ),
                                op.metadata(),
                            );
                        }

                        result = self.resolve_type(data, data.get_type(method).returns());
                        bytecode.push(IR::new(Operation::Invoke, [*name, *arity, n]));
                    } else if self.functions.contains_key(&self.resolve(*name)) {
                    let name = if let Some(fqn) = self.aliases.get(&name) {
                        *fqn
                    } else {
                        *name
                    };
                        let kind;
                    if let Some(&func) = self.functions.get(&name) {
                        let fn_type = func;

                        for idx in 0..*arity {
                            let sub = self.substitute_type(data, fn_type.get(idx));
                            if !self.match_type(data, self.peek(idx), sub) {
                                self.error(format!(
                                    "Argument #{} of function '{}' does not match expected type {:?}, got {:?}",
                                    arity - idx ,
                                    data.symbol_name(op.operands()[0]),
                                    data.get_type(func.get(idx)).output(data), 
                                    data.get_type(self.peek(idx)).output(data)
                                ), op.metadata());
                            }
                        }

                        kind = self.resolve_type(data, func.returns());
                    } else {
                        let fn_type = *data.get_type(op.kind());
                        let func = data.find_type(fn_type);

                        for idx in 0..*arity {
                            let sub = self.substitute_type(data, fn_type.get(idx));
                            if !self.match_type(data, self.peek(idx), sub) {
                                self.error(format!(
                                    "Argument #{} of function '{}' does not match expected type {:?}, got {:?}",
                                    arity - idx ,
                                    data.symbol_name(op.operands()[0]),
                                    data.get_type(self.resolve_type(data, data.get_type(func).get(idx))).output(data), 
                                    data.get_type(self.resolve_type(data, self.peek(idx))).output(data)
                                ), op.metadata());
                            }
                        }

                        kind = self.resolve_type(data, fn_type.returns());
                    }

                        result = kind;

                        // let func_constant = data.add_constant(func_constant, op.kind());
                        // bytecode.push(IR::new(Operation::Const, [func_constant, 0, 0]));
                        bytecode.push(IR::new(Operation::Call, [name, arity + 1, 0]));

                    } else {
                        self.warn(format!("Unknown function '{}' used in method-like calling convention", data.symbol_name(*name)), op.metadata());
                    }

                    self.pop(1 + arity);
                    self.push(result);

                    ip += 1;
                    continue;
                }
                Operation::Closure => {
                    let [name, len, ..] = op.operands();

                    self.functions.insert(*name, *data.get_type(op.kind()));

                    self.expects_return = Some(self.resolve_type(data, data.get_type(op.kind()).returns()));
                    let body = &mut self.check(&code[ip + 1..ip + 1+ len], data); 

                    bytecode.push(IR::new(Operation::Closure, [*name, body.len(), 0]));
                    bytecode.append(body);
                    

                    ip += len + 1;
                    continue;
                }
                Operation::Function => {
                    let [name, arity, len] = op.operands();

                    self.functions.insert(*name, *data.get_type(op.kind()));

                    self.expects_return = Some(self.resolve_type(data, data.get_type(op.kind()).returns()));
                    let body =&mut self.check(&code[ip + 1..ip + 1+ len], data); 

                    bytecode.push(IR::new(Operation::Function, [*name, *arity, body.len()]));
                    bytecode.append(body);
                    

                    ip += len + 1;
                    continue;
                }
                Operation::Method => {
                    let &[mut symbol, arity, len] = op.operands();

                    let original_symbol = symbol;

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
                    self.expects_return = Some(data.get_type(op.kind()).returns());
                    let mut body = self.check(&code[ip + 1..ip + 1 + len], data);
                    bytecode.push(IR::new(Operation::Method, [original_symbol, arity, body.len()]));
                    bytecode.append(&mut body);

                    self.expects_return = None;
                    self.scope = scope;
                    ip += len + 1;
                    continue;
                }
                Operation::Leave => {
                    if let Some(expected) = self.expects_return {
                        let sub_expected = self.substitute_type(data, expected);
                        if !self.match_type(data, self.peek(0), sub_expected) && (self.peek(0) != 0) {
                            self.error(format!(
                                "Expected to return '{}' but it has branch that returns '{}'",
                                data.get_type(self.resolve_type(data, sub_expected)).output(data),
                                data.get_type(self.resolve_type(data, self.peek(0))).output(data),
                            ), op.metadata());
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
