pub mod passes;
use common::error::ErrorOrigin;
use rand::{Rng, distr::Alphanumeric, rng};

use std::collections::HashMap;

use common::interner::Interner;
use common::program::Program;
use common::symbols::SymbolTable;
use common::{Value, ValueKind};
use common::{
    error::Error,
    opcodes::{Byte, Code, IR, Operation},
};

pub trait CompilationPass {
    fn compile<'compilation>(
        &mut self,
        code: &Vec<Code>,
        constants: &mut Interner<Value>,
        symbols: &mut SymbolTable,
    ) -> Result<Vec<Code>, Error>;
}

#[derive(Default)]
pub struct Compiler<'compilation> {
    pipeline: Vec<&'compilation mut dyn CompilationPass>,
    constants: Interner<Value>,
    symbols: SymbolTable,

    labels: HashMap<String, usize>,
}

impl<'compilation> Compiler<'compilation> {
    pub fn label(&mut self, name: String) -> usize {
        if self.labels.contains_key(&name) {
            panic!("Unable to redefine label");
        }

        let symbol = self.symbols.insert(name.to_owned(), None);
        self.labels.insert(name, symbol);

        symbol
    }

    pub fn random_label(&mut self) -> String {
            rand::rng()
                .sample_iter(&Alphanumeric)
                .take(8)
                .map(char::from)
                .collect()
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

            bytecode.append(&mut match op.code() {
                Operation::Noop => continue,
                Operation::Pop => vec![Code::new(Byte::Pop)],
                Operation::Const => {
                    vec![Code::new_with_operands(Byte::Push, op.operands().to_vec())]
                }
                Operation::Add => vec![Code::new(Byte::Add)],
                Operation::Print => vec![Code::new_with_operands(Byte::Print, vec![if op.operands()[0] == 1 { 1 } else { 0 }])],
                Operation::Leave => vec![Code::new(Byte::Leave)],
                Operation::Function => {
                    let mut result = vec![
                        // Code::new(Byte::Halt),
                    ];
                    let [name, arity, len] = op.operands();
                    let label = if let Some(name) = self.symbols.name(*name) {
                        self.label(name.to_owned())

                    } else {
                        return Err(Error::new(common::error::ErrorOrigin::COMPILE, "Unable to lookup function name".to_string()));
                    };

                    skips += len;

                    let chunk = &code[cursor..(cursor + len)];
                    let constant = match self.do_compile(chunk) {
                        Ok(mut body) => {
                            body.push(Code::new_with_operands(Byte::Push, vec![0]));
                            body.push(Code::new(Byte::Leave));
                            result.push(Code::new_with_operands(Byte::Label, vec![label, body.len()]));
                            result.append(&mut body);
                            self.constants.intern(Value::new(ValueKind::FUNCTION(*arity, label)))
                        }
                        Err(err) => {
                            return Err(err);
                        }
                    };

                    if let Some(symbol) = self.symbols.name(*name) {
                        self.symbols.insert(symbol.to_owned(), Some(constant))
                    } else {
                        return Err(Error::new(
                            common::error::ErrorOrigin::COMPILE,
                            format!(
                                "Unable to compile function '{}'",
                                self.symbols
                                    .name(*name)
                                    .unwrap_or(&"<unknown>".to_string())
                            ),
                        ));
                    };

                    let mut func = Code::new(Byte::Push);
                    func.with_operands(vec![constant]);
                    result.push(func);
                    // Halt the machine as the VM should not reach this section directly, I think?
                    // vec![
                    //     //Code::new(Byte::Halt), 
                    //     Code::new_with_operands(Byte::Label, vec![label]), func]

                    result
                }
                Operation::Argument => {
                    let mut byte = Code::new(Byte::Peek);
                    let operands = op.operands();

                    byte.with_operands(vec![operands[0], operands[2]]);

                    vec![byte]
                }
                Operation::Load => {
                    vec![Code::new_with_operands(Byte::Load, vec![op.operands()[0]])]
                },
                Operation::Call => {
                    let mut code = vec![];
                    let [symbol, declaration_arity, _] = op.operands();
                    match self.symbols.constant(*symbol).map(|const_| {
                        *self.constants
                            .lookup(*const_)
                            .cloned()
                            .unwrap_or_default()
                            .kind()
                            // .clone()
                    }) {
                        Some(ValueKind::FUNCTION(definition_arity, _)) => {
                            if *declaration_arity != definition_arity {
                                return Err(Error::new(common::error::ErrorOrigin::COMPILE, format!("Function '{}' called with {} arguments, while expecting {}", self.symbols.name(*symbol).cloned().unwrap_or("<unknown>".to_string()), definition_arity, declaration_arity)));
                            }
                            
                            if let Some(constant) = self.symbols.constant(*symbol) {
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
                // Operation::Condition => {
                //     let mut result = vec![];
                //     if let (Some(condition_offset), Some(body_offset), Some(alternative_offset)) = (op.get(0), op.get(1), op.get(2)) {
                //             let mut code = Program::new(code.code()[cursor..cursor+**offset].to_vec());
                //             code.with_constants(program.get_constants());
                //             code.with_symbols(program.symbols());
                //             let jump;
                //             let else_;
                //             if let Ok(output) = self.compile(code) {
                //                 result.append(&mut output.code().to_vec());
                //                 program.with_symbols(output.symbols());
                //                 program.with_constants(output.get_constants());
                //
                //                 if idx == 0 {
                //                     result.push(Code::new_with_operands(Byte::Jumpz, vec![]));
                //                     jump = result.len() - 1;
                //                 } else if idx == 2 {
                //                     else_ = result.len() - 1;
                //                 }
                //             }
                //
                //     
                //
                //         // cursor += condition_offset;
                //         // let body_code = self.compile(Program::new(code.code()[cursor..cursor+condition_offset].to_vec()));
                //         // cursor += body_offset;
                //         // dbg!(condition_offset, body_offset);
                //     }
                //
                //     result
                // }
                _ => todo!("Unable to compile {:?}", op.code()),
            });
        }



        Ok(bytecode)
    }

    pub fn compile(&mut self, code: Program<IR>) -> Result<Program<Code>, Error> {
        let mut program = Program::new(vec![]);
        self.symbols = code.symbols();
        self.constants = Interner::from(code.get_constants());

        match self.do_compile(code.code()) {
            Ok(mut bytecode) => {
                for compiler in &mut self.pipeline {
                    bytecode = if let Ok(code) = compiler.compile(&bytecode, &mut self.constants, &mut self.symbols) {
                        code
                    } else {
                        return Err(Error::new(ErrorOrigin::COMPILE, "Unable to compile".to_string()));
                    }
                }

                program.with_code(bytecode)
            },
            Err(e) => return Err(e),
        };


        program.with_constants(self.constants.dump());
        program.with_symbols(self.symbols.clone());


        let mut bytes = program.code().to_vec();
        bytes.push(Code::new(Byte::Leave));
        program.with_code(bytes);

        Ok(program)
    }
}
