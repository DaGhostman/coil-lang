mod frame;
pub mod options;
mod utils;

use std::io::{stderr, stdout};

use common::memory2::Memory;
use common::program::Program;
use common::Value;
use common::{
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
    ValueKind,
};
use frame::Frame;
use options::MachineOptions;
use utils::output::Output;

// struct Suspension {
//     program: Program<Byte>,
//     memory: Memory,
// }

// #[derive(PartialEq, Debug)]
// pub enum ExecutionStatus {
//     Complete,
//     Halt,
//     Pause,
// }

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    memento: Option<Program<Code>>,

    ip: usize,
    memory: Memory<Value>,
    frame: Frame,

    options: MachineOptions,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            memento: None,
            ip: 0,
            memory: Memory::new(32),
            frame: Frame::default(),
            options: MachineOptions::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
        }
    }
}

impl Machine {
    pub fn with_options(options: MachineOptions) -> Self {
        let mut this = Self::default();
        this.options = options;
        this.stdout = Output::new(&this.options, || Box::new(stdout().lock()));
        this.stderr = Output::new(&this.options, || Box::new(stderr().lock()));

        this
    }

    fn call(&mut self, ip: usize, arity: usize) {}

    fn execute(&mut self, code: Program<Code>) -> Result<ValueKind, Error> {
        self.memory.import_constants(code.get_constants());

        while let Some(op) = code.get(self.ip) {
            // eprintln!("#{:0>8} {:?}\t{:?}", self.ip, op.byte(), self.memory);
            match op.byte() {
                Byte::Call => {}
                Byte::Halt => {
                    self.halt = true;
                }
                Byte::Enter => self.enter(None),
                Byte::Leave => self.leave(),
                Byte::Push => {
                    if let Some(constant) = op.operand(0) {
                        // dbg!(&code.get_constants());
                        if let Err(e) = self.memory.push(constant) {
                            match e {
                                common::memory2::MemoryError::StackOverflow => {
                                    return Err(Error::new(
                                        ErrorOrigin::RUNTIME,
                                        "Stackoverflow".to_string(),
                                    ))
                                }
                                common::memory2::MemoryError::StackUnderflow => {
                                    return Err(Error::new(
                                        ErrorOrigin::RUNTIME,
                                        "Stackunderflow".to_string(),
                                    ))
                                }
                            }
                        }
                    } else {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            String::from("Opcode doesn't have operand"),
                        ));
                    }
                }
                Byte::Pop => {
                    self.memory.pop();
                }
                Byte::Add => {
                    let rhs = self.memory.pop_value().map(|v| v.kind()).cloned();
                    let lhs = self.memory.pop_value().map(|v| v.kind()).cloned();
                    let result = self.memory.define(match (lhs, rhs) {
                        (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs + rhs))
                        }
                        (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs + rhs as f64))
                        }
                        (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs as f64 + rhs))
                        }
                        (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                            Value::new(ValueKind::INTEGER(lhs.wrapping_add(rhs)))
                        }
                        _ => {
                            return Err(Error::new(
                                ErrorOrigin::RUNTIME,
                                String::from("Operands do not match any valid types"),
                            ));
                        }
                    });

                    let _ = self.memory.push(result);
                }
                Byte::Print => {
                    self.stdout
                        .write(match self.memory.pop_value().map(|v| v.kind()) {
                            Some(ValueKind::NONE) => String::new(),
                            Some(ValueKind::BOOLEAN(bit)) => format!("{}", bit),
                            Some(ValueKind::INTEGER(num)) => format!("{}", num),
                            Some(ValueKind::FLOAT(num)) => format!("{:.?}", num),
                            // Some(Some(ValueKind::STRING(num))) => {
                            //     if let Some(str) = code.string(num) {
                            //         format!("{}", str)
                            //     } else {
                            //         String::new()
                            //     }
                            // }
                            // Some(ValueKind::RANGE(start, end)) => format!("{}..{}", start, end),
                            // Some(ValueKind::ARRAY(key)) => {
                            //     if let Some(Object::Array(arr)) = self.memory.lookup(key) {
                            //         let mut formatted = "[".to_string();
                            //         let count = min(3, arr.len());
                            //         for i in 0..count {
                            //             if let Some(item) = arr.item(count - (count - i)) {
                            //                 formatted = format!("{}{}, ", formatted, item);
                            //             }
                            //         }
                            //         if count < arr.len() {
                            //             formatted = format!("{}...", formatted);
                            //         }
                            //
                            //         formatted = format!("{}]", formatted);
                            //
                            //         formatted
                            //     } else {
                            //         String::new()
                            //     }
                            // }
                            a => {
                                dbg!(a);
                                String::new()
                            }
                        });

                    if op.operand(0).is_some() {
                        self.stdout.write("\n".to_string());
                    }
                }

                _ => (),
            }

            if self.halt {
                break;
            }

            // if self.memory.size() > self.options.memory().limit() {
            //     return Err(Error::new(
            //         ErrorOrigin::RUNTIME,
            //         format!(
            //             "Ran out of memory, current usage is {} bytes out of maximum allowed {} bytes",
            //             self.memory.size(),
            //             self.options.memory().limit(),
            //         ),
            //     ));
            // }

            self.ip += 1;
        }

        if self.halt {
            return Ok(ValueKind::NONE);
        }
        if self.memento.is_some() {
            return Ok(ValueKind::NONE);
        }

        // let result = if !self.halt && self.memento.is_none() {
        //     if self.memory.len() == 0 {
        //         ValueKind::default()
        //     } else {
        //         *self.memory.pop().unwrap_or_default().kind()
        //     }
        // } else {
        //     ValueKind::NONE
        // };
        Ok(self
            .memory
            .pop_value()
            .map(|v| v.kind().clone())
            .unwrap_or_default())
    }

    pub fn resume(&mut self) -> Result<ValueKind, Error> {
        if let Some(program) = self.memento.take() {
            self.memento = None;
            self.execute(program)
        } else {
            Err(Error::new(
                ErrorOrigin::RUNTIME,
                "Attempting to resume a non-suspended program".to_string(),
            ))
        }
    }

    pub fn run(&mut self, code: Program<Code>) -> Result<ValueKind, Error> {
        self.execute(code)
    }

    pub fn enter(&mut self, stack_offset: Option<usize>) {
        let frame = self.frame.to_owned();
        self.frame = Frame::new(
            self.ip,
            self.memory.stack_size() - stack_offset.unwrap_or(0),
        );
        self.frame.with_parent(frame);
    }

    pub fn leave(&mut self) {
        let f = self.frame.parent();
        let val = self.memory.pop();

        self.memory.truncate(self.frame.stack());
        if let Some(k) = val {
            if let Err(e) = self.memory.push(k) {
                eprintln!("ERR: {:?}", e);
            }
        }

        if !self.frame.is_scoped() {
            self.ip = self.frame.tell();
        }
        self.frame = f.cloned().unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use common::{
        interner::Interner,
        opcodes::{Byte, Code},
        Value, ValueKind,
    };
    use parser::program::Program;

    use crate::Machine;

    #[test]
    fn test_integer_addition() {
        let mut values = Interner::default();
        let num = values.intern(Value::new(ValueKind::INTEGER(2)));
        let mut constant = Code::new(Byte::Push);
        constant.with_operands(vec![num]);

        let mut program = Program::new(vec![
            constant.clone(),
            constant.clone(),
            Code::new(Byte::Add),
        ]);
        program.with_constants(values.dump());
        let result = Machine::default().run(program);

        assert!(result.is_ok());
        assert_eq!(result, Ok(ValueKind::INTEGER(4)));
    }

    #[test]
    fn test_float_addition() {
        let mut values = Interner::default();
        let a = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::FLOAT(0.8)))],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::FLOAT(0.1)))],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_constants(values.dump());

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(ValueKind::FLOAT(0.9)));
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Interner::default();
        let a = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::INTEGER(8)))],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::FLOAT(0.1)))],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_constants(values.dump());

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(ValueKind::FLOAT(8.1)));
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Interner::default();
        let a = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::FLOAT(0.8)))],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            vec![values.intern(Value::new(ValueKind::INTEGER(1)))],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_constants(values.dump());

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(ValueKind::FLOAT(1.8)));
    }
}
