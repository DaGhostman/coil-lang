pub mod passes;
use common::error::ErrorOrigin;
use common::interner::Interner;
use common::program::data::Data;
use rand::{Rng, distr::Alphanumeric};

use rustc_hash::FxHashMap as HashMap;

use common::Value;
use common::program::program::Program;
use common::{
    error::Error,
    opcodes::{Byte, Code, IR, Operation},
};

pub trait CompilationPass {
    fn compile<'compilation>(
        &mut self,
        code: &'compilation [Code],
        data: &mut Data,
    ) -> Result<Vec<Code>, Error>;
}

#[derive(Default)]
pub struct Compiler<'compilation> {
    pipeline: Vec<&'compilation mut dyn CompilationPass>,

    data: Data,

    label_prefix: Option<String>,
    labels: Interner<String>,
    // labels: HashMap<String, usize>,
    context: Context,
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Variable {
    pub(crate) position: usize,
    pub(crate) scope: usize,
    pub(crate) readonly: bool,
    pub(crate) assigned: bool,
}

#[derive(Default, Clone, Debug)]
pub(crate) struct Variables {
    storage: HashMap<usize, Variable>,
    scope: usize,
}

impl Variables {
    pub fn enter(&mut self) {
        self.scope += 1;
    }

    pub fn leave(&mut self) {
        self.scope -= 1;
    }

    pub fn create(&mut self, symbol: usize) -> usize {
        let length = self.storage.len();
        self.storage
            .entry(symbol)
            .or_insert_with(|| Variable {
                position: length,
                scope: self.scope,
                readonly: false,
                assigned: false,
            })
            .position
    }

    pub fn seal(&mut self, symbol: usize) {
        let length = self.storage.len();
        self.storage
            .entry(symbol)
            .and_modify(|v| {
                v.readonly = true;
            })
            .or_insert_with(|| Variable {
                position: length,
                scope: self.scope,
                readonly: true,
                assigned: true,
            });
    }

    pub fn assign(&mut self, symbol: usize) {
        let length = self.storage.len();
        self.storage
            .entry(symbol)
            .and_modify(|v| {
                v.assigned = true;
            })
            .or_insert_with(|| Variable {
                position: length,
                scope: self.scope,
                readonly: false,
                assigned: true,
            });
    }

    pub fn is_sealed(&mut self, symbol: &usize) -> bool {
        self.storage
            .get(symbol)
            .filter(|v| v.readonly && v.assigned)
            .is_some()
    }

    pub fn available(&self, symbol: usize) -> bool {
        self.storage
            .get(&symbol)
            .filter(|v| v.position <= self.scope)
            .is_some()
    }

    pub fn has(&self, symbol: &usize) -> bool {
        self.storage.contains_key(symbol)
            && self
                .storage
                .get(symbol)
                .map(|v| v.scope <= self.scope)
                .unwrap()
                == true
    }

    pub fn get(&self, symbol: usize) -> Variable {
        self.storage.get(&symbol).copied().unwrap_or_default()
    }
    pub fn get_mut(&mut self, symbol: usize) -> Option<&mut Variable> {
        self.storage.get_mut(&symbol)
    }
}

#[derive(Default, Clone)]
pub(crate) struct ClassDefinition {
    state: Vec<(usize, usize)>,
    methods: HashMap<usize, (usize, usize)>
}

impl ClassDefinition {
    pub fn add_method(&mut self, name: usize, label: usize, arity: usize) {
        self.methods.insert(name, (label, arity));
    }

    pub fn add_prop(&mut self, name: usize, type_: usize) {
        self.state.push((name, type_));
    }

    pub fn extend(&mut self, source: &Self) {
        self.state.extend(&source.state);
        self.methods.extend(&source.methods);
    }
}

#[derive(Default)]
pub(crate) struct Context {
    frame: usize,
    variables: Vec<Variables>,
    classes: HashMap<usize, ClassDefinition>,
}

impl Context {
    pub fn enter(&mut self) {
        self.frame += 1;
    }

    pub fn leave(&mut self) {
        self.frame -= 1;
    }

    pub fn get_upvalue(&self, symbol: usize) -> Variable {
        self.variables[self.frame - 1].get(symbol)
    }

    pub fn upvalue(&mut self, symbol: usize) -> usize {
        let upvalue = self.variables[self.frame - 1].get(symbol);
        self.variables[self.frame].create(symbol);
        if let Some(var) = self.variables[self.frame].get_mut(symbol) {
            var.assigned = upvalue.assigned;
            var.readonly = upvalue.readonly;
        }

        upvalue.position
    }

    pub fn frame(&self) -> usize {
        self.frame - 1
    }

    pub fn variables(&mut self) -> &mut Variables {
        if self.variables.len() <= self.frame {
            self.variables.resize(self.frame + 4, Default::default());
        }

        &mut self.variables[self.frame]
    }

    pub fn define_class(&mut self, name: usize) {
        self.classes.insert(name, Default::default());
    }

    pub fn add_method(&mut self, owner: usize, name: usize, label: usize, arity: usize) {
        self.classes.entry(owner).and_modify(|entry| {
            entry.add_method(name, label, arity);
        });
    }

    pub fn add_property(&mut self, owner: usize, name: usize, type_: usize) {
        self.classes.entry(owner).and_modify(|entrty| {
            entrty.add_prop(name, type_);
        });
    }

    pub fn extend(&mut self, target: usize, source: usize) {
        if let (Some(mut t), Some(source)) = (self.classes.get(&target).cloned(), self.classes.get(&source)) {
            t.extend(source);
            self.classes.insert(target, t);
        }
    }

    pub fn get_fields(&self, owner: usize) -> Option<Vec<usize>> {
        self.classes.get(&owner).map(|c| c.state.iter().map(|(n, _)| *n).collect())
    }
}

impl<'compilation> Compiler<'compilation> {
    pub fn label(&mut self, name: String) -> usize {
        let key = if let Some(prefix) = self.label_prefix.take() {
            format!("{}::{}", prefix, name)
        } else {
            name
        };

        self.data.add_symbol(key, None)
    }

    pub fn random_label(&mut self) -> String {
        format!(
            "@{}",
            rand::rng()
                .sample_iter(&Alphanumeric)
                .take(8)
                .map(char::from)
                .collect::<String>()
        )
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
            // eprintln!("#{} {:?}", cursor, op.code());

            bytecode.append(&mut match op.code() {
                Operation::Begin => {
                    self.context.variables().enter();
                    vec![]
                },
                Operation::End => {
                    self.context.variables().leave();
                    vec![]
                },
                Operation::Noop => continue,
                Operation::Pop => vec![Code::new(Byte::Pop)],
                Operation::Const => {
                    vec![Code::new_with_operands(Byte::Push, op.operands().to_vec())]
                }
                Operation::Not => vec![Code::new(Byte::Not)],
                Operation::Add => vec![Code::new(Byte::Add)],
                Operation::Subtract => vec![Code::new(Byte::Sub)],
                Operation::Divide => vec![Code::new(Byte::Div)],
                Operation::Multiply => vec![Code::new(Byte::Mul)],
                Operation::Modulo => vec![Code::new(Byte::Mod)],
                Operation::Less => vec![Code::new(Byte::Less)],
                Operation::LessEqual => vec![Code::new(Byte::Greater), Code::new(Byte::Not)],
                Operation::Greater => vec![Code::new(Byte::Greater)],
                Operation::GreaterEqual => vec![Code::new(Byte::Less), Code::new(Byte::Not)],
                Operation::Print => vec![Code::new_with_operands(Byte::Print, vec![if op.operands()[0] == 1 { 1 } else { 0 }])],
                Operation::Leave => vec![Code::new(Byte::Leave)],
                Operation::Function => {
                    let mut result = vec![];
                    let [name, arity, len, ..] = op.operands();
                    let label = if let Some(name) = self.data.symbol_name(*name) {
                        self.label(name.to_owned())

                    } else {
                        return Err(Error::new(common::error::ErrorOrigin::COMPILE, "Unable to lookup function name".to_string()));
                    };

                    skips += len;

                    let constant = self.data.add_constant((Value::FUNCTION(*arity, label)));
                    if let Some(symbol) = self.data.symbol_name(*name) {
                        self.data.add_symbol(symbol.to_owned(), Some(constant))
                    } else {
                        return Err(Error::new(
                            common::error::ErrorOrigin::COMPILE,
                            format!(
                                "Unable to compile function '{}'",
                                self.data
                                    .symbol_name(*name)
                                    .unwrap_or(&"<unknown>".to_string())
                            ),
                        ));
                    };


                    let chunk = &code[cursor..(cursor + len)];
                    self.context.enter();
                    match self.do_compile(chunk) {
                        Ok(mut body) => {
                            body.push(Code::new_with_operands(Byte::Push, vec![self.data.add_constant(Value::NONE)]));
                            body.push(Code::new(Byte::Leave));

                            result.push(Code::new_with_operands(Byte::Label, vec![label, body.len()]));
                            result.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    };
                    self.context.leave();

                    let mut func = Code::new(Byte::Push);
                    func.with_operands(vec![constant]);
                    result.push(func);

                    result
                }
                Operation::Assign => {
                    let operands = op.operands();
                    if !self.context.variables().has(&operands[0]) {
                        return Err(Error::new(ErrorOrigin::COMPILE, "Unable to assign to non-existing variable".to_string()))
                    } else if self.context.variables().is_sealed(&operands[0]) {
                        return Err(Error::new(ErrorOrigin::COMPILE, "Assigning to a constant variable is not allowed".to_string()))
                    }

                    self.context.variables().assign(operands[0]);

                    vec![Code::new_with_operands(Byte::Store, vec![
                        self.context.variables().create(operands[0])
                    ])]
                }
                Operation::Declare => {
                    let operands = op.operands();

                    if operands[1] != 0 {
                        self.context.variables().seal(operands[0]);
                    }

                    vec![Code::new_with_operands(Byte::Store, vec![
                        self.context.variables().create(operands[0])
                    ])]
                }
                Operation::Upvalue => {
                    let operands = op.operands();
                    let variable = self.context.variables().create(operands[0]);
                    let upvalue = self.context.upvalue(operands[0]);


                    vec![
                        Code::new_with_operands(Byte::Upvalue, vec![self.context.frame(), variable, upvalue])
                    ]
                }
                Operation::Argument => {
                    let operands = op.operands();
                    let position = self.context.variables().create(operands[0]);

                    vec![Code::new_with_operands(Byte::Peek, vec![position, operands[2]])]
                }
                Operation::Load => {
                    let operands = op.operands();

                    if !self.context.variables().has(&operands[0]) {
                        return Err(Error::new(ErrorOrigin::COMPILE, format!("Variable '{}' does not exist.", self.data.symbol_name(operands[0]).cloned().unwrap_or("unknown".to_string()))));
                    } else if !self.context.variables().available(operands[0]) {
                        return Err(Error::new(ErrorOrigin::COMPILE, format!("Variable '{}' is defined in lower scope than the current one.", self.data.symbol_name(operands[0]).cloned().unwrap_or("unknown".to_string()))));
                    } 

                    vec![Code::new_with_operands(Byte::Load, vec![
                        self.context.variables().get(operands[0]).position
                    ])]
                },
                Operation::Call => {
                    let mut code = vec![];
                    let [symbol, declaration_arity, ..] = op.operands();
                    match self.data.symbol_constant(*symbol).map(|const_| {
                        self.data.constant(*const_)
                            .copied()
                            .unwrap_or_default()
                    }) {
                        Some(Value::FUNCTION(definition_arity, _)) => {
                            if *declaration_arity != definition_arity {
                                return Err(Error::new(common::error::ErrorOrigin::COMPILE, format!("Function '{}' called with {} arguments, while expecting {}", self.data.symbol_name(*symbol).unwrap_or(&"<unknown>".to_string()), definition_arity, declaration_arity)));
                            } else if let Some(constant) = self.data.symbol_constant(*symbol) {
                                code.push(Code::new_with_operands(Byte::Push, vec![*constant]));


                                code.push(Code::new_with_operands(Byte::Call, vec![*declaration_arity]));
                            }
                        }
                        None => {
                            code.push(Code::new_with_operands(Byte::Jump, vec![*symbol]));
                            if let Some(constant) = self.data.symbol_constant(*symbol) {
                                code.push(Code::new_with_operands(Byte::Push, vec![*constant]));


                                code.push(Code::new_with_operands(Byte::Call, vec![*declaration_arity]));
                            }
                        }
                        a => {
                            dbg!(a);
                            panic!("DBG!!");
                        }
                    }

                    code
                }
                Operation::Condition => {
                    let mut result = vec![];
                    if let Some(condition_length) = op.get(0) {
                        let mut local_cursor = cursor;

                        let mut condition = self.do_compile(&code[local_cursor..local_cursor+condition_length])?;
                        local_cursor += condition_length;
                        skips += condition_length;

                        result.append(&mut condition);
                        let rand = self.random_label();

                        let then_label = self.label(rand);
                        let rand = self.random_label();
                        let else_label = self.label(rand);
                        let rand = self.random_label();
                        let outside_label = self.label(rand);


                        result.push(Code::new_with_operands(Byte::Jumpz, vec![then_label, else_label]));

                        if let Some(body_length) = op.get(1) {
                            let mut chunk = self.do_compile(&code[local_cursor..=local_cursor + body_length])?;
                            let size = chunk.len();

                            local_cursor += body_length;
                            skips += body_length;

                            result.push(Code::new_with_operands(Byte::Jump, vec![then_label]));
                            result.push(Code::new_with_operands(Byte::Label, vec![then_label, size]));
                            // result.push(Code::new(Byte::Scope));
                            result.append(&mut chunk);
                            result.push(Code::new_with_operands(Byte::Jump, vec![outside_label]));
                            if let Some(alternative_length) = op.get(2) {
                                let mut chunk = self.do_compile(&code[local_cursor..local_cursor + alternative_length])?;
                                skips += alternative_length;

                                result.push(Code::new_with_operands(Byte::Label, vec![else_label, chunk.len()]));
                                // result.push(Code::new(Byte::Scope));
                                result.append(&mut chunk);
                                result.push(Code::new_with_operands(Byte::Label, vec![outside_label, 0]));
                            }
                        }
                    }

                    result
                }
                Operation::BitAnd | Operation::And => vec![Code::new(Byte::And)],
                Operation::BitOr | Operation::Or => vec![Code::new(Byte::Or)],
                Operation::BitXor | Operation::Xor => vec![Code::new(Byte::Xor)],
                Operation::Range => vec![Code::new(Byte::Range)],
                Operation::Array => vec![Code::new_with_operands(Byte::Array, vec![*op.get(0).unwrap()])],
                Operation::Prop => {
                    let [owner, name, ..] = op.operands();
                    self.context.add_property(*owner, *name, 0);
                    vec![]
                    // vec![Code::new_with_operands(Byte::Prop, vec![*owner, *name])]
                },
                Operation::Method => {
                    let mut result = vec![];
                    let [owner, name, arity, len, ..] = op.operands();
                    let label = if let Some(name) = self.data.symbol_name(*name) {
                        self.label(name.to_owned())
                    } else {
                        return Err(Error::new(ErrorOrigin::COMPILE, "Unable to lookup method name".to_string()));
                    };

                    skips += len;
                    
                    self.context.enter();
                    match self.do_compile(&code[cursor..cursor+len]) {
                        Ok(mut body) => {
                            body.push(Code::new_with_operands(Byte::Push, vec![self.data.add_constant(Value::default())]));
                            body.push(Code::new(Byte::Leave));

                            result.insert(0, Code::new_with_operands(Byte::Label, vec![label, body.len()]));
                            result.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }
                    self.context.leave();

                    self.context.add_method(*owner, *name, label, *arity);
                    result.insert(0, Code::new_with_operands(Byte::Method, vec![*owner, *name, label, *arity]));

                    result
                }
                Operation::Class => {
                    let mut class = vec![];
                    let label_prefix = self.label_prefix.take();
                    self.label_prefix = Some(self.random_label());

                    let [owner, len, ..] = op.operands();
                    match self.do_compile(&code[cursor..cursor+len]) {
                        Ok(mut body) => {
                            // class.push(Code::new_with_operands(Byte::Class, vec![*owner, body.len()]));
                            class.append(&mut body);
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    }

                    self.context.define_class(*owner);


                    self.label_prefix = label_prefix;
                    class
                }
                Operation::Instantiate => {
                    if let Some(mut fields) = self.context.get_fields(op.operands()[0]) {
                        fields.insert(0, op.operands()[0]);

                        vec![Code::new_with_operands(Byte::Instantiate, fields)]
                    } else {
                        return Err(Error::new(ErrorOrigin::COMPILE, "Unable to instantiate unknown object".to_string()));
                    }
                }
                Operation::Invoke => vec![Code::new_with_operands(Byte::Invoke, op.operands().to_vec())],
                Operation::This => vec![Code::new(Byte::This)],
                Operation::PropAssign => {
                    let mut operands = op.operands().clone();
                    operands[1] = 1;


                    vec![Code::new_with_operands(Byte::Prop, operands[0..2].to_vec())]
                }

                Operation::PropLoad => {
                    let operands = op.operands();

                    vec![Code::new_with_operands(Byte::Prop, operands[0..2].to_vec())]
                }
                _ => todo!("Unable to compile {:?}", op.code()),
            });
        }

        Ok(bytecode)
    }

    pub fn compile(&mut self, code: Program<IR>) -> Result<Program<Code>, Error> {
        let mut program = Program::new(vec![]);
        self.data = code.data().clone();
        self.data.add_constant(Value::NONE);

        // self.context.enter();
        match self.do_compile(code.code()) {
            Ok(mut bytecode) => {
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

                program.with_code(bytecode)
            }
            Err(e) => return Err(e),
        };
        // self.context.leave();

        let void = self.data.add_constant(Value::default());
        program.with_data(self.data.clone());

        let mut bytes = program.code().to_vec();
        bytes.insert(0, Code::new_with_operands(Byte::Push, vec![void]));
        bytes.push(Code::new(Byte::Leave));
        program.with_code(bytes);

        Ok(program)
    }
}
