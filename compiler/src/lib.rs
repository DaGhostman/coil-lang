pub mod passes;
use common::error::ErrorOrigin;
use common::program::data::Data;
use common::types::{Kind, Type};
use rand::{Rng, distr::Alphanumeric};

use rustc_hash::FxHashMap as HashMap;

use common::Value;
use common::program::program::Program;
use common::{
    error::Error,
    opcodes::{Byte, Code, IR, Operation},
};

pub trait CompilationPass {
    fn compile(&mut self, code: &[Code], data: &mut Data) -> Result<Vec<Code>, Error>;
}

pub struct Compiler<'compilation> {
    pipeline: Vec<&'compilation mut dyn CompilationPass>,
    data: Data,
    context: Context,
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Variable {
    pub(crate) position: usize,
    pub(crate) readonly: bool,
    pub(crate) assigned: bool,
    pub(crate) r#type: usize,
}

#[derive(Default, Clone, Debug)]
pub(crate) struct Variables {
    storage: HashMap<(usize, usize), Variable>,
    scope: usize,
    scopes: Vec<usize>,
}

impl Variables {
    pub fn enter(&mut self) {
        self.scope += 1;
        self.scopes.push(self.storage.len());
    }

    pub fn leave(&mut self) -> usize {
        self.scope -= 1;

        if let Some(size) = self.scopes.pop() {
            self.storage
                .retain(|(slot, _), var| var.position <= size && *slot <= self.scope);
            size - self.storage.len()
        } else {
            unreachable!("There are no more scopes available");
        }
    }

    pub fn variables_in_scope(&self) -> usize {
        self.storage
            .iter()
            .filter_map(|((scope, _), _)| {
                if self.scope == *scope {
                    Some(scope)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .len()
    }

    pub fn clear(&mut self) -> usize {
        let len = self.storage.len();
        self.storage.clear();

        len
    }

    pub fn create(&mut self, symbol: usize, r#type: usize) -> usize {
        let position = self.storage.len();

        // dbg!((
        //     self.scope,
        //     symbol,
        //     self.storage.contains_key(&(self.scope, symbol)),
        //     self.storage.len(),
        // ));

        // debug_assert!(!self.has(symbol));

        self.storage
            .entry((self.scope, symbol))
            .and_modify(|entry| {
                entry.r#type = r#type;
                entry.position = position;
            })
            .or_insert_with(|| Variable {
                position,
                readonly: false,
                assigned: false,
                r#type,
            })
            .position
    }

    pub fn seal(&mut self, symbol: usize) {
        let position = self.storage.len();

        self.storage
            .entry((self.scope, symbol))
            .and_modify(|v| {
                v.readonly = true;
            })
            .or_insert_with(|| Variable {
                position,
                readonly: true,
                assigned: true,
                r#type: 0,
            });
    }

    pub fn assign(&mut self, symbol: usize) {
        // let position = self.storage.len();
        debug_assert!(self.has(symbol));
        if let Some(var) = self.get_mut(symbol) {
            var.assigned = true;
        }

        // dbg!(symbol);
        // self.storage
        //     .entry((self.scope, symbol))
        //     .and_modify(|v| {
        //         v.assigned = true;
        //     })
        //     .or_insert_with(|| Variable {
        //         position,
        //         readonly: false,
        //         assigned: true,
        //         r#type: 0,
        //     });
    }

    pub fn is_sealed(&mut self, symbol: usize) -> bool {
        self.storage
            .get(&(self.scope, symbol))
            .filter(|v| v.readonly && v.assigned)
            .is_some()
    }

    pub fn available(&self, symbol: usize) -> bool {
        !self
            .storage
            .keys()
            .filter(|(scope, name)| *scope <= self.scope && name == &symbol)
            .collect::<Vec<_>>()
            .is_empty()
    }

    pub fn has(&self, symbol: usize) -> bool {
        // dbg!((self.scope, symbol, self.storage.keys()));
        !self
            .storage
            .keys()
            .filter(|(slot, value)| &self.scope >= slot && &symbol == value)
            .collect::<Vec<_>>()
            .is_empty()
    }

    pub fn get(&self, symbol: usize) -> Variable {
        let mut scope = self
            .storage
            .keys()
            .filter_map(|(slot, name)| {
                if self.scope >= *slot && *name == symbol {
                    Some((*slot, *name))
                } else {
                    None
                }
            })
            .collect::<Vec<(usize, usize)>>();

        scope.sort_by(|(lhs, ..), (rhs, ..)| lhs.cmp(rhs));

        if scope.is_empty() {
            unreachable!("Attempting to access invalid symbol");
        }

        self.storage[&scope[0]]
    }
    pub fn get_mut(&mut self, symbol: usize) -> Option<&mut Variable> {
        let scope = self
            .storage
            .keys()
            .filter_map(|(slot, name)| {
                if self.scope >= *slot && *name == symbol {
                    Some((*slot, *name))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if scope.is_empty() {
            unreachable!("Attempting to access invalid symbol");
        }
        let key = scope.first().unwrap();

        self.storage.get_mut(key)
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ClassDefinition {
    state: Vec<(usize, usize)>,
    methods: HashMap<usize, (usize, bool)>,
}

impl ClassDefinition {
    pub fn add_method(&mut self, name: usize, label: usize, public: bool) {
        self.methods.insert(name, (label, public));
    }

    pub fn add_prop(&mut self, name: usize, type_: usize) {
        self.state.push((name, type_));
    }

    pub fn extend(&mut self, source: &Self) {
        self.state.extend(&source.state);
        self.methods.extend(&source.methods);
    }
}

#[derive(Default, Debug)]
pub(crate) struct InterfaceDefinition {
    methods: HashMap<usize, Vec<IR>>,
}

impl InterfaceDefinition {
    pub fn add_method(&mut self, name: usize, body: Vec<IR>) {
        self.methods.insert(name, body);
    }
}

#[derive(Debug, Default)]
pub(crate) struct Context {
    tco: Vec<bool>,
    current: Vec<usize>,
    frame: usize,
    variables: Variables,

    classes: HashMap<usize, ClassDefinition>,
    interfaces: HashMap<usize, InterfaceDefinition>,
}

impl Context {
    pub fn prefix(&self, data: &Data) -> String {
        self.current
            .iter()
            .map(|v| data.symbol_name(*v).clone())
            .collect::<Vec<String>>()
            .join("::")
    }

    pub fn enter(&mut self, scope: usize) {
        // println!("{}Entering: {scope}", "-".repeat(self.current.len()));
        self.tco.push(false);
        self.current.push(scope);
        self.variables.enter();
        self.frame += 1;
    }

    pub fn current(&self) -> Option<&usize> {
        self.current.last()
    }

    pub fn parent(&self) -> Option<&usize> {
        if self.frame == 0 {
            None
        } else {
            self.current.get(self.frame - 1)
        }
    }

    pub fn leave(&mut self) -> usize {
        // let scope = self.current.pop();
        // println!(
        //     "{}Leaving: {}",
        //     "-".repeat(self.current.len()),
        //     scope.unwrap_or(42069)
        // );
        self.current.pop();
        self.tco.pop();
        if self.frame > 0 {
            self.frame -= 1;
        }
        self.variables.leave()
    }

    pub fn tell(&self) -> usize {
        self.variables.variables_in_scope()
    }

    pub fn set_tco(&mut self, state: bool) {
        if let Some(current) = self.tco.last_mut() {
            *current = state;
        }
    }

    pub fn is_tco(&mut self) -> bool {
        if let Some(state) = self.tco.last() {
            *state
        } else {
            false
        }
    }

    pub fn get_upvalue(&self, symbol: usize) -> Variable {
        self.variables.get(symbol)
    }

    pub fn upvalue(&mut self, symbol: usize) -> (usize, usize) {
        let upvalue = self.variables.get(symbol);

        let position = self.variables.create(symbol, upvalue.r#type);
        if let Some(var) = self.variables.get_mut(symbol) {
            var.assigned = upvalue.assigned;
            var.readonly = upvalue.readonly;
        }

        (upvalue.position, position)
    }

    pub fn frame(&self) -> usize {
        self.frame - 1
    }

    pub fn variables(&mut self) -> &mut Variables {
        &mut self.variables
    }

    pub fn define_class(&mut self, name: usize) {
        self.classes.insert(name, Default::default());
    }

    pub fn add_method(&mut self, public: bool, owner: usize, name: usize, label: usize) {
        self.classes.entry(owner).and_modify(|entry| {
            entry.add_method(name, label, public);
        });
    }

    pub fn add_property(&mut self, owner: usize, name: usize, type_: usize) {
        self.classes.entry(owner).and_modify(|entrty| {
            entrty.add_prop(name, type_);
        });
    }

    pub fn define_interface(&mut self, name: usize) {
        self.interfaces.insert(name, Default::default());
    }

    pub fn add_contract(&mut self, owner: usize, name: usize, body: Vec<IR>) {
        self.interfaces.entry(owner).and_modify(|entry| {
            entry.add_method(name, body);
        });
    }
}

impl<'compilation> Compiler<'compilation> {
    #[must_use] pub fn new(data: Data) -> Self {
        Self {
            data,
            context: Context::default(),
            pipeline: Vec::with_capacity(4),
        }
    }
    pub fn label(&mut self, name: Option<String>) -> usize {
        let key = format!(
            "@{}{}::{}",
            self.context.prefix(&self.data),
            name.map(|v| v.to_string()).unwrap_or_default(),
            self.random_label()
        );

        self.data.add_symbol(key, None)
    }

    pub fn random_label(&mut self) -> String {
        rand::rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(char::from)
            .collect::<String>()
            .to_string()
    }

    pub fn attach(&mut self, pass: &'compilation mut dyn CompilationPass) {
        self.pipeline.push(pass);
    }

    fn do_compile(&mut self, code: &[IR]) -> Result<Vec<Code>, Error> {
        let mut bytecode = vec![];
        let mut skips = 0;
        let mut cursor = 0;

        for op in code {
            cursor += 1;
            if skips > 0 {
                skips -= 1;
                continue;
            }
            // eprintln!("#{cursor:0>8}\t{:?}", op.code());

            bytecode.append(&mut match op.code() {
                Operation::Begin => {
                    // println!("Name: {}", self.data.symbol_name(op.get(0)));
                    self.context.enter(op.get(0));
                    vec![]
                }
                Operation::End => {
                    let vars = self.context.tell();
                    self.context.leave();
                    vec![Code::new_with_operands(Byte::Pop, [vars, 0, 0])]
                }
                Operation::Noop => continue,
                Operation::Pop => vec![Code::new_with_operands(Byte::Pop, [1, 0, 0])],

                Operation::Const => {
                    vec![Code::new_with_operands(Byte::Push, *op.operands())]
                }
                Operation::Not => vec![Code::new(Byte::Not)],
                Operation::Add => vec![Code::new(Byte::Add)],
                Operation::Subtract => vec![Code::new(Byte::Sub)],
                Operation::Divide => vec![Code::new(Byte::Div)],
                Operation::Multiply => vec![Code::new(Byte::Mul)],
                Operation::Pow => vec![Code::new(Byte::Pow)],
                Operation::Modulo => vec![Code::new(Byte::Mod)],
                Operation::Less => vec![Code::new(Byte::Less)],
                Operation::LessEqual => vec![Code::new(Byte::LessEqual)],
                Operation::Greater => vec![Code::new(Byte::Greater)],
                Operation::GreaterEqual => vec![Code::new(Byte::GreaterEqual)],
                Operation::Equal => vec![Code::new(Byte::Equal)],
                Operation::Print => vec![Code::new_with_operands(
                    Byte::Print,
                    [usize::from(op.operands()[0] == 1), 0, 0],
                )],
                Operation::Inc => {
                    let is_prefix = op.get(1) == 1;
                    let mut result = vec![];

                    let r#type = self.data.add_type(Type::integer());
                    let increment = self.data.add_constant(Value::INTEGER(1), r#type);

                    if !is_prefix {
                        result.push(Code::new_with_operands(
                            Byte::Load,
                            [self.context.variables().get(op.get(0)).position, 0, 0],
                        ));
                    }

                    result.push(Code::new_with_operands(Byte::Push, [increment, 0, 0]));
                    result.push(Code::new_with_operands(
                        Byte::Load,
                        [self.context.variables().get(op.get(0)).position, 0, 0],
                    ));
                    result.push(Code::new(Byte::Add));
                    result.push(Code::new_with_operands(
                        Byte::Store,
                        [self.context.variables().get(op.get(0)).position, 0, 0],
                    ));
                    if is_prefix {
                        result.push(Code::new_with_operands(
                            Byte::Load,
                            [self.context.variables().get(op.get(0)).position, 0, 0],
                        ));
                    }
                    result
                }
                Operation::Dec => {
                    let is_prefix = op.get(1) == 1;
                    let mut result = vec![];

                    let r#type = self.data.add_type(Type::integer());
                    let increment = self.data.add_constant(Value::INTEGER(1), r#type);

                    if !is_prefix {
                        result.push(Code::new_with_operands(
                            Byte::Load,
                            [self.context.variables().get(op.get(0)).position, 0, 0],
                        ));
                    }

                    result.push(Code::new_with_operands(Byte::Push, [increment, 0, 0]));
                    result.push(Code::new_with_operands(
                        Byte::Load,
                        [self.context.variables().get(op.get(0)).position, 0, 0],
                    ));
                    result.push(Code::new(Byte::Sub));
                    result.push(Code::new_with_operands(
                        Byte::Store,
                        [self.context.variables().get(op.get(0)).position, 0, 0],
                    ));
                    if is_prefix {
                        result.push(Code::new_with_operands(
                            Byte::Load,
                            [self.context.variables().get(op.get(0)).position, 0, 0],
                        ));
                    }
                    result
                }
                Operation::TypeOf => vec![Code::new(Byte::TypeOf)],
                Operation::Negate => vec![Code::new(Byte::Negate)],
                Operation::Leave => vec![Code::new(Byte::Leave)],
                Operation::Closure => {
                    let mut result = vec![];
                    let [name, length, ..] = op.operands();
                    let mut func = vec![];

                    if let Ok(mut function) = self.do_compile(&code[cursor..cursor + length]) {
                        func.append(&mut function);
                    }

                    let outside = self.random_label();
                    let skip = self.data.add_symbol(outside, None);

                    skips += length;
                    result.push(Code::new_with_operands(Byte::Jump, [skip, 0, 0]));

                    func.insert(func.len() - 1, Code::new_with_operands(Byte::Label, [skip, 0, 0]));
                    result.append(&mut func);


                    result
                }
                Operation::Function => {
                    let mut result = vec![];
                    let [name, arity, len, ..] = op.operands();

                    let label = self.label(None);

                    skips += len;

                    let constant = self
                        .data
                        .add_constant(Value::FUNCTION(*arity, label), op.kind());
                    let symbol = self.data.symbol_name(*name);
                    self.data.add_symbol(symbol.to_owned(), Some(constant));

                    // dbg!(&code[cursor..], self.data.symbol_name(*name));
                    let chunk = &code[cursor..cursor + len];

                    match self.do_compile(chunk) {
                        Ok(mut body) => {
                            let ty = self.data.add_type(Type::void());
                            body.push(Code::new_with_operands(
                                Byte::Push,
                                [self.data.add_constant(Value::NONE, ty), 0, 0],
                            ));
                            body.push(Code::new(Byte::Leave));

                            result.push(Code::new_with_operands(Byte::Label, [label, 0, 0]));
                            result.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }

                    let mut func = Code::new(Byte::Push);
                    func.with_operands([constant, 0, 0]);
                    func.with_type(op.kind());
                    result.push(func);

                    result
                }
                Operation::Assign => {
                    let operands = op.operands();
                    if !self.context.variables().has(operands[0]) {
                        return Err(Error::new(
                            ErrorOrigin::COMPILE,
                            "Unable to assign to non-existing variable".to_string(),
                        ));
                    } else if self.context.variables().is_sealed(operands[0]) {
                        return Err(Error::new(
                            ErrorOrigin::COMPILE,
                            "Assigning to a constant variable is not allowed".to_string(),
                        ));
                    }

                    self.context.variables().assign(operands[0]);

                    vec![Code::new_with_operands(
                        Byte::Store,
                        [self.context.variables().get(operands[0]).position, 0, 0],
                    )]
                }

                Operation::Declare | Operation::Argument => {
                    let [name, readonly, ..] = op.operands();

                    self.context.variables().create(*name, op.kind());
                    // dbg!((self.data.symbol_name(*name), name, var, self.context.tell()));
                    if *readonly != 0 {
                        self.context.variables().seal(*name);
                    }

                    vec![]
                }
                Operation::Upvalue => {
                    let operands = op.operands();
                    let (upvalue, variable) = self.context.upvalue(operands[0]);

                    vec![Code::new_with_operands(
                        Byte::Upvalue,
                        [self.context.frame(), variable, upvalue],
                    )]
                }
                Operation::Load => {
                    let [name, ..] = op.operands();

                    if !self.context.variables().has(*name) {
                        return Err(Error::new(
                            ErrorOrigin::COMPILE,
                            format!(
                                "Variable '{}' does not exist.",
                                self.data.symbol_name(*name)
                            ),
                        ));
                    } else if !self.context.variables().available(*name) {
                        return Err(Error::new(
                            ErrorOrigin::COMPILE,
                            format!(
                                "Variable '{}' is defined in lower scope than the current one.",
                                self.data.symbol_name(*name)
                            ),
                        ));
                    }


                    let mut result;
                    if self.context.variables().is_sealed(*name) {
                        // We are loading a constant
                        let constant = self.data.symbol_constant(*name);
                        result = vec![Code::new_with_operands(Byte::Push, [constant, 0, 0])];
                    } else {
                        let var = self.context.variables().get(*name);
                        let mut load = Code::new_with_operands(Byte::Load, [var.position, 0, 0]);

                        load.with_type(var.r#type);
                        result = vec![load];
                    }

                    result
                }
                Operation::Yield => {
                    vec![Code::new(Byte::Yield)]
                }
                Operation::Call => {
                    let mut result = vec![];
                    let [symbol, call_arity, ..] = op.operands();

                    if self.data.symbol_constant_exists(*symbol) {
                        let const_ = self.data.symbol_constant(*symbol);


                        if let Value::FUNCTION(definition_arity, _) = self.data.constant(const_) {
                            if call_arity == definition_arity {
                                let constant = self.data.symbol_constant(*symbol);
                                // if self.context.current() == Some(&symbol)
                                //     && code[cursor].code() == Operation::Leave
                                // {
                                //     self.context.set_tco(true);
                                //
                                //     result.push(Code::new_with_operands(Byte::Jump, [*symbol, 0, 0]));
                                // } else {
                                    result.push(Code::new_with_operands(Byte::Push, [constant, 0, 0]));
                                    let mut call =
                                        Code::new_with_operands(Byte::Call, [*call_arity, 0, 0]);

                                    call.with_type(self.data.symbol_constant_type(*symbol).returns());

                                    result.push(call);
                                // }
                            } else {
                                return Err(Error::new(
                                    common::error::ErrorOrigin::COMPILE,
                                    format!(
                                        "Function '{}' called with {} arguments, while expecting {}",
                                        self.data.symbol_name(*symbol),
                                        call_arity,
                                        definition_arity,
                                    ),
                                ));
                            }
                        }
                    } else if self.context.variables.has(*symbol) {
                        let variable = self.context.variables().get(*symbol);

                        result.push(Code::new_with_operands(Byte::Load, [variable.position, 0, 0]));
                        let mut call = Code::new_with_operands(Byte::Call, [*call_arity, 0, 0]);
                        call.with_type(self.data.get_type(variable.r#type).returns());
                        result.push(call);
                    } else {
                        panic!("Unable to call '{}' as function as it does not exist", self.data.symbol_name(*symbol));
                    }


                    result
                }
                Operation::Iterate => {
                    let mut result = vec![];
                    let [name, length, ..] = op.operands();
                    skips += length;


                    // let ty = self.data.add_type(Type::new(Kind::List(0)));
                    let inside_label = self.label(None);
                    let outside_label = self.label(None);

                    // let rand = self.random_label();
                    // let rand_symbol = self.data.add_symbol(rand, None);
                    // let iterator = self
                    //     .context
                    //     .variables()
                    //     .create(rand_symbol, self.data.add_type(Type::void()));
                    let rand = self.random_label();
                    let iterator = self.context.variables().create(
                        self.data.add_symbol(rand, None),
                        self.data.add_type(Type::void()),
                    );
                    // result.push(Code::new_with_operands(
                    //     Byte::Push,
                    //     [self.data.add_constant(Value::ITERATOR(0), ty), 0, 0],
                    // ));
                    // result.push(Code::new_with_operands(Byte::Store, [iterator, 0, 0]));

                    let position = self.context.variables().create(*name, op.kind());

                    // dbg!(iterator, self.data.symbol_name(rand_symbol));
                    // dbg!(position, self.data.symbol_name(*name));
                    // result.push(Code::new_with_operands(Byte::Store, [iterator, 0, 0]));
                    result.push(Code::new_with_operands(
                        Byte::Iterator,
                        [position, iterator, 0],
                    ));
                    result.push(Code::new_with_operands(Byte::Label, [inside_label, 0, 0]));
                    result.push(Code::new_with_operands(
                        Byte::Iterate,
                        [outside_label, position, iterator],
                    ));

                    // result.push(Code::new_with_operands(Byte::Store, [position, 0, 0]));

                    // let rand = self.random_label();
                    // let iter = self.context.variables().create(
                    //     self.data.add_symbol(rand, None),
                    //     self.data.add_type(Type::void()),
                    // );
                    // let position = self.context.variables().create(*name, op.kind()) + 2;
                    //
                    let mut body = self.do_compile(&code[cursor..cursor + length])?;

                    //
                    // let ty = self.data.add_type(Type::new(Kind::List(0)));
                    // result.push(Code::new_with_operands(
                    //     Byte::Push,
                    //     [self.data.add_constant(Value::ITERATOR(0), ty), 0, 0],
                    // ));
                    // result.push(Code::new(Byte::Duplicate));
                    // result.push(Code::new_with_operands(Byte::Store, [iter, 0, 0]));
                    //
                    // result.push(Code::new_with_operands(Byte::Load, [iterator, 0, 0]));
                    // result.push(Code::new_with_operands(Byte::Load, [iter, 0, 0]));
                    // result.push(Code::new_with_operands(
                    //     Byte::Iterate,
                    //     [outside_label, position, iter],
                    // ));
                    // result.push(Code::new_with_operands(Byte::Store, [position, 0, 0]));
                    result.append(&mut body);
                    // result.push(Code::new_with_operands(Byte::Pop, [1, 0, 0]));
                    // result.push(Code::new_with_operands(
                    //     Byte::Pop,
                    //     [self.context.variables().leave(), 0, 0],
                    // ));
                    result.push(Code::new_with_operands(Byte::Jump, [inside_label, 0, 0]));
                    result.push(Code::new_with_operands(Byte::Label, [outside_label, 0, 0]));

                    // self.context.variables().leave();
                    // result.push(Code::new_with_operands(Byte::Pop, [3, 0, 0]));

                    result
                }
                Operation::Loop => {
                    let [condition_length, body_length, _] = op.operands();
                    let mut result = vec![];
                    // let condition_length = op.get(0);
                    let inside_label = self.label(None);
                    let outside_label = self.label(None);

                    skips += condition_length;
                    let mut local_cursor = cursor;
                    result.push(Code::new_with_operands(Byte::Label, [inside_label, 0, 0]));
                    result.append(
                        &mut self
                            .do_compile(&code[local_cursor..local_cursor + condition_length])?,
                    );

                    local_cursor += condition_length;

                    result.push(Code::new_with_operands(Byte::Jumpz, [outside_label, 0, 0]));

                    // let body_length = op.get(1);
                    skips += body_length;

                    let mut chunk =
                        self.do_compile(&code[local_cursor..local_cursor + body_length])?;

                    result.append(&mut chunk);
                    result.push(Code::new_with_operands(Byte::Jump, [inside_label, 0, 0]));
                    result.push(Code::new_with_operands(Byte::Label, [outside_label, 0, 0]));

                    result
                }
                Operation::Condition => {
                    let [condition_length, body_length, alternative_length] = op.operands();
                    let mut result = vec![];
                    // let condition_length = op.get(0);
                    let mut local_cursor = cursor;

                    let mut condition =
                        self.do_compile(&code[local_cursor..local_cursor + condition_length])?;
                    local_cursor += condition_length;
                    skips += condition_length;

                    result.append(&mut condition);

                    let then_label = self.label(None);
                    let else_label = self.label(None);
                    let outside_label = self.label(None);

                    result.push(Code::new_with_operands(Byte::Jumpz, [else_label, 0, 0]));

                    // let body_length = op.get(1);
                    let mut chunk =
                        self.do_compile(&code[local_cursor..=local_cursor + body_length])?;

                    local_cursor += body_length;
                    skips += body_length;

                    result.push(Code::new_with_operands(Byte::Label, [then_label, 0, 0]));
                    result.append(&mut chunk);
                    result.push(Code::new_with_operands(Byte::Jump, [outside_label, 0, 0]));
                    // let alternative_length = op.get(2);
                    let mut chunk =
                        self.do_compile(&code[local_cursor..local_cursor + alternative_length])?;
                    skips += alternative_length;

                    result.push(Code::new_with_operands(Byte::Label, [else_label, 0, 0]));
                    result.append(&mut chunk);
                    result.push(Code::new_with_operands(Byte::Label, [outside_label, 0, 0]));

                    result
                }
                Operation::BitAnd | Operation::And => vec![Code::new(Byte::And)],
                Operation::BitOr | Operation::Or => vec![Code::new(Byte::Or)],
                Operation::BitXor | Operation::Xor => vec![Code::new(Byte::Xor)],
                Operation::Range => vec![Code::new_with_operands(Byte::Range, *op.operands())],
                Operation::Array => vec![Code::new_with_operands(Byte::Array, [op.get(0), 0, 0])],
                Operation::Prop => {
                    let [owner, name, ..] = op.operands();
                    self.context.add_property(*owner, *name, 0);
                    // let owner = op.operands()[0];
                    vec![Code::new_with_operands(
                        Byte::Prop,
                        [*owner, op.operands()[1], *name],
                    )]
                }
                Operation::Method => {
                    let mut result = vec![];
                    let &[mut symbol, _, len] = op.operands();

                    let public = (symbol & 1) == 1;
                    symbol >>= 1;
                    let name = symbol & 0xffff;
                    symbol >>= 16;
                    let owner = symbol;

                    let label = if self.context.classes[&owner].methods.contains_key(&name) {
                        self.context.classes[&owner].methods[&name].0
                    } else {
                        self.label(Some(self.data.symbol_name(name).to_owned()))
                    };

                    skips += len + 1;

                    self.context.add_method(public, owner, name, label);
                    // self.context.enter(name);
                    // let ty = self.data.add_type(Type::object(owner));
                    // let this = self.data.add_symbol("this".to_string(), None);
                    // self.context.variables().create(this, ty);
                    // dbg!(self.context.variables().seal(this));
                    // let var = self.context.variables().get(this).position;
                    // result.push(Code::new_with_operands(Byte::Load, [var, 0, 0]));
                    match self.do_compile(&code[cursor..=(cursor + len)]) {
                        Ok(mut body) => {
                            let ty = self.data.add_type(Type::void());
                            body.push(Code::new_with_operands(
                                Byte::Push,
                                [self.data.add_constant(Value::default(), ty), 0, 0],
                            ));
                            body.push(Code::new(Byte::Leave));

                            result.insert(0, Code::new_with_operands(Byte::Label, [label, 0, 0]));
                            result.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }

                    result
                }
                Operation::Implement => {
                    let mut implementation = vec![];
                    let [interface, class, len] = op.operands();

                    let methods = self.context.interfaces[interface].methods.clone();

                    for (name, method) in methods {
                        if let Ok(mut body) = self.do_compile(&method) {
                            if let Some(method) = body.first_mut() {
                                self.context
                                    .add_method(true, *class, name, method.operand(2));
                                *method = Code::new_with_operands(
                                    Byte::Method,
                                    [*class, method.operand(1), method.operand(2)],
                                );
                            }

                            implementation.append(&mut body);
                        }
                    }

                    // Continue parsing the actual body
                    if let Ok(mut body) = self.do_compile(&code[cursor..cursor + len]) {
                        implementation.append(&mut body);
                        skips += len;
                    }


                    implementation
                }
                Operation::Interface => {
                    let mut interface = vec![];
                    let [owner, len, ..] = op.operands();
                    self.context.define_interface(*owner);

                    for (idx, byte) in code[cursor..cursor + len].iter().enumerate() {
                        if byte.code() == Operation::Method {
                            let [owner, name, mlen, ..] = op.operands();
                            self.context.add_contract(
                                *owner,
                                *name,
                                code[idx..idx + mlen].to_vec(),
                            );
                        }
                    }
                    skips += len;
                    interface
                }
                Operation::Class => {
                    let mut class = vec![];

                    let [owner, len, ..] = op.operands();
                    self.context.define_class(*owner);
                    skips += len;


                    match self.do_compile(&code[cursor..cursor + len]) {
                        Ok(mut body) => {
                            class.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }

                    class
                }
                Operation::Instantiate => {
                    let mut result = vec![];
                    let [name, arity, ..] = op.operands();
                    let mut code = Code::new_with_operands(Byte::Instantiate, [*name, *arity, 0]);
                    code.with_type(self.data.add_type(Type::new(Kind::Object(*name))));

                    result.push(code);
                    let constructor = self.data.add_symbol("new".to_string(), None);
                    if self.context.classes[name]
                        .methods
                        .contains_key(&constructor)
                    {
                        let (label, _) = self.context.classes[name].methods[&constructor];
                        result.push(Code::new_with_operands(Byte::Invoke, [label, *arity, 0]));
                    }

                    result
                }
                Operation::Invoke => {
                    let &[name, mut arity, ..] = op.operands();
                    arity += 1;
                    let owner = self.context.current().copied().unwrap_or(usize::MAX);

                    // dbg!(self.data.symbol_name(owner), self.data.symbol_name(name));
                    if self.context.classes.contains_key(&owner) {


                        let (label, public) = self.context.classes[&owner].methods[&name];

                        if !public {
                            if let Some(current) = self.context.current() {
                                if *current != owner {
                                    println!("Calling a private method is forbidden");
                                }
                            }
                        }
                        vec![Code::new_with_operands(Byte::Invoke, [label, arity, 0])]
                    } else {
                        vec![Code::new_with_operands(Byte::Invoke, [name, arity, 0])]
                    }

                }
                Operation::Bind => {
                    let mut t = Code::new(Byte::This);

                    if let Some(class) = self.context.parent() {
                        // this.with_type(self.data.add_type(Type::new(Kind::Object(*class))));
                        t.with_type(self.data.add_type(Type::new(Kind::Object(*class))));
                    }
                    self.context
                        .variables()
                        .create(self.data.add_symbol("this".to_string(), None), t.get_type());

                    vec![]
                }
                Operation::This => {
                    let mut t = Code::new_with_operands(Byte::Load, [0, 0, 0]);

                    if let Some(class) = self.context.parent() {
                        t.with_type(self.data.add_type(Type::new(Kind::Object(*class))));
                    }

                    vec![t]
                }
                _ => todo!("Unable to compile {:?}", op.code()),
            });
        }

        Ok(bytecode)
    }

    pub fn compile(&mut self, code: &Program<IR>) -> Result<(Program<Code>, Data), Error> {
        let mut program = Program::new(vec![]);
        let code = code.code();

        let symbol = self.data.add_symbol("root".to_string(), None);

        self.context.enter(symbol);

        match self.do_compile(code) {
            Ok(mut bytecode) => {
                #[cfg(feature = "dump")]
                {
                    eprintln!("------ Source compilation");
                    eprintln!("{:->64}", ' ');
                    for (cursor, c) in bytecode.iter().enumerate() {
                        eprintln!(
                            "#{:0>8} | {: >12} | {: >32}",
                            cursor,
                            format!("{:?}", c.byte()).to_uppercase(),
                            match c.byte() {
                                Byte::Push => format!("{:?}", self.data.constant(c.operand(0))),
                                Byte::Pop => format!("{}", c.operand(0)),
                                Byte::Call | Byte::Invoke => format!(
                                    "{}({})",
                                    self.data.symbol_name(c.operand(0)),
                                    c.operand(1)
                                ),
                                Byte::Load | Byte::Store | Byte::Peek => {
                                    format!("@{}", c.operand(0))
                                }
                                Byte::Jump | Byte::Label => {
                                    format!("{}", self.data.symbol_name(c.operand(0)))
                                }
                                Byte::Jumpz => {
                                    format!("{}", self.data.symbol_name(c.operand(0)),)
                                }
                                _ => String::new(),
                            },
                        )
                    }
                }

                let entrypoint = self.data.symbol_index("main".to_string());
                let alt = self.data.symbol_index("@#$%".to_string());
                if entrypoint == alt {
                    return Err(Error::new(
                        ErrorOrigin::COMPILE,
                        "Missing main function".to_string(),
                    ));
                }
                let entrypoint_constant = self.data.symbol_constant_value(entrypoint);
                if let Value::FUNCTION(_, _) = entrypoint_constant {
                    bytecode.insert(0, Code::new_with_operands(Byte::Call, [0, 1, 0]));
                    bytecode.insert(
                        0,
                        Code::new_with_operands(
                            Byte::Push,
                            [self.data.symbol_constant(entrypoint), 0, 0],
                        ),
                    );
                } else {
                    return Err(Error::new(
                        ErrorOrigin::COMPILE,
                        "Main label exists, but is not a function".to_string(),
                    ));
                }

                for compiler in &mut self.pipeline {
                    bytecode = if let Ok(code) = compiler.compile(&bytecode, &mut self.data) {
                        code
                    } else {
                        return Err(Error::new(
                            ErrorOrigin::COMPILE,
                            "Unable to compile".to_string(),
                        ));
                    }
                }

                program.with_code(bytecode);
            }
            Err(e) => return Err(e),
        }

        #[cfg(feature = "dump")]
        {
            eprintln!("\n{:->64}", ' ');
            eprintln!("------ Optimized compilation");
            eprintln!("{:->64}", ' ');
            for (cursor, c) in program.code().iter().enumerate() {
                eprintln!(
                    "#{:0>8} | {: >12} | {: >32}",
                    cursor,
                    format!("{:?}", c.byte()).to_uppercase(),
                    match c.byte() {
                        Byte::Push => format!("{:?}", self.data.constant(c.operand(0))),
                        Byte::Pop => format!("{}", c.operand(0)),
                        // Byte::Call =>
                        //     format!("={}({})", self.data.symbol_name(c.operand(0)), c.operand(1)),
                        Byte::Load | Byte::Store | Byte::Peek => {
                            format!("@{}", c.operand(0))
                        }
                        Byte::Jump | Byte::Jumpz | Byte::Invoke => {
                            format!("={}", c.operand(0))
                        }
                        Byte::Jumpr => {
                            format!("+{}", c.operand(0))
                        }
                        Byte::Leave
                        | Byte::Load
                        | Byte::Store
                        | Byte::Peek
                        | Byte::Call
                        | Byte::Invoke
                        | Byte::Print
                        | Byte::Add
                        | Byte::Sub
                        | Byte::Mul
                        | Byte::Div
                        | Byte::Greater
                        | Byte::Equal
                        | Byte::Less
                        | Byte::Not => String::new(),
                        _ => format!("Missing format"),
                    },
                )
            }
        }

        Ok((program, self.data.clone()))
    }
}
