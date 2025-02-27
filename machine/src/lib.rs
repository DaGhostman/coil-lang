mod frame;
pub mod options;
mod utils;

use ahash::AHashMap as HashMap;
use std::io::{stderr, stdout};

use common::Value;
use common::memory2::Memory;
use common::program::Program;
use common::{
    ValueKind,
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
};
use frame::Frame;
use options::MachineOptions;
use utils::output::Output;

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    ip: usize,
    memory: Memory<Value>,
    frame: Vec<Frame>,

    labels: HashMap<usize, usize>,

    options: MachineOptions,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            ip: 0,
            memory: Memory::new(),
            frame: Vec::with_capacity(32),
            options: MachineOptions::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
            labels: HashMap::default(),
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

    fn execute(&mut self, code: Program<Code>) -> Result<(), Error> {
        self.ip = 0;
        while let Some(op) = code.get(self.ip) {
            // eprintln!(
            //     "#{:0>8} {:?}\t{:?}",
            //     self.ip,
            //     op.byte(),
            //     self.memory.stack_values()
            // );
            match op.byte() {
                Byte::Label => {
                    if let (Some(label), Some(size)) = (op.operand(0), op.operand(1)) {
                        self.labels.insert(*label, self.ip);
                        self.ip += size;
                    }
                }
                Byte::Call => {
                    if let Some(func) = self.memory.pop_value().map(|value| value.kind()).copied() {
                        if let ValueKind::FUNCTION(arity, label) = func {
                            self.enter(arity);
                            if let Some(position) = self.labels.get(&label) {
                                self.ip = *position;
                            }
                        }
                    } else {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            "Unable to invoke function as it does not exists".to_string(),
                        ));
                    }
                }
                Byte::Halt => {
                    self.halt = true;
                }
                Byte::Enter => self.enter(0),
                Byte::Leave => self.leave(),
                Byte::Push => {
                    if let Some(constant) = op.operand(0) {
                        if let Err(e) = self.memory.push(*constant) {
                            match e {
                                common::memory2::MemoryError::StackOverflow => {
                                    return Err(Error::new(
                                        ErrorOrigin::RUNTIME,
                                        "Stackoverflow".to_string(),
                                    ));
                                }
                                common::memory2::MemoryError::StackUnderflow => {
                                    return Err(Error::new(
                                        ErrorOrigin::RUNTIME,
                                        "Stackunderflow".to_string(),
                                    ));
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
                Byte::Peek => {
                    if let [Some(symbol), Some(offset)] = [op.operand(0), op.operand(1)] {
                        if let Some(frame) = self.frame.last_mut() {
                            let size = frame.stack();
                            frame.store(symbol, size + offset);
                        } else {
                            panic!("NO FRAME");
                        }
                    }
                }
                Byte::Load => {
                    if let Some(name) = op.operand(0) {
                        if let Some(frame) = self.frame.last_mut() {
                            if let Some(position) = frame.lookup(name) {
                                let value = self.memory.peek(*position);
                                if let Err(e) = self.memory.push(*value) {
                                    return Err(Error::new(
                                        ErrorOrigin::RUNTIME,
                                        match e {
                                            common::memory2::MemoryError::StackOverflow => {
                                                "Stack overflow".to_string()
                                            }
                                            common::memory2::MemoryError::StackUnderflow => {
                                                "Stack underflow".to_string()
                                            }
                                        },
                                    ));
                                }
                            } else {
                                return Err(Error::new(
                                    ErrorOrigin::RUNTIME,
                                    "Undefined variable".to_string(),
                                ));
                            }
                        } else {
                            return Err(Error::new(
                                ErrorOrigin::RUNTIME,
                                "No call frame available".to_string(),
                            ));
                        }
                    } else {
                        return Err(Error::new(
                            ErrorOrigin::RUNTIME,
                            "Missing operand".to_string(),
                        ));
                    }
                }
                Byte::Less => {
                    let rhs = self.memory.pop_value().map(|v| *v.kind());
                    let lhs = self.memory.pop_value().map(|v| *v.kind());

                    let result = match (lhs, rhs) {
                        (Some(ValueKind::INTEGER(l)), Some(ValueKind::INTEGER(r))) => {
                            ValueKind::BOOLEAN(l < r)
                        }
                        (a, b) => {
                            dbg!(a, b);
                            panic!("Unsupported values");
                        }
                    };

                    let constant = self.memory.define(Value::new(result));
                    let _ = self.memory.push(constant);
                }
                Byte::Add => {
                    let lhs = self.memory.pop_value().map(|v| *v.kind());
                    let rhs = self.memory.pop_value().map(|v| *v.kind());
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
                        a => {
                            dbg!(&a);
                            return Err(Error::new(
                                ErrorOrigin::RUNTIME,
                                String::from("Operands do not match any valid types"),
                            ));
                        }
                    });

                    let _ = self.memory.push(result);
                }
                Byte::Subtract => {
                    let lhs = self.memory.pop_value().map(|v| *v.kind());
                    let rhs = self.memory.pop_value().map(|v| *v.kind());
                    let result = self.memory.define(match (lhs, rhs) {
                        (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs - rhs))
                        }
                        (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::FLOAT(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs - rhs as f64))
                        }
                        (Some(ValueKind::FLOAT(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                            Value::new(ValueKind::FLOAT(lhs as f64 - rhs))
                        }
                        (Some(ValueKind::INTEGER(rhs)), Some(ValueKind::INTEGER(lhs))) => {
                            Value::new(ValueKind::INTEGER(lhs.wrapping_sub(rhs)))
                        }
                        a => {
                            dbg!(&a);
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
                            Some(ValueKind::FUNCTION(_, symbol)) => format!("fn({})", symbol),
                            // Some(ValueKind::STRING(v)) => format!("str({})", v),
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
                                // dbg!(a);
                                String::new()
                            }
                        });

                    if op.operand(0).is_some() && op.operand(0).unwrap() == &1 {
                        self.stdout.write("\n".to_string());
                    }
                }
                Byte::Jump => {
                    if let Some(label) = op.operand(0) {
                        if let Some(position) = self.labels.get(&label) {
                            self.ip = *position;
                        }
                    }
                }
                Byte::Jumpz => {
                    let value = self.memory.pop_value().map(|ev| ev.kind());
                    if !matches!(value, Some(ValueKind::BOOLEAN(_))) {
                        eprintln!("Condition does not evaluate to boolean, using fallback checks");
                    }

                    let state = match value {
                        Some(ValueKind::NONE) => false,
                        Some(ValueKind::BOOLEAN(byte)) => *byte,
                        Some(ValueKind::FLOAT(value)) => *value != 0.0,
                        Some(ValueKind::INTEGER(value)) => *value != 0,
                        _ => panic!("Unable to evaluate"),
                    };

                    let (then, branch) = (
                        op.operand(0).expect("Missing jump label"),
                        op.operand(1).expect("Missing alternate label"),
                    );

                    if let Some(v) = self.labels.get(if state { then } else { branch }) {
                        self.ip = *v;
                    } else {
                        panic!("Unknown label");
                    }
                }
                // Byte::Scope => {
                //     if let Some(current) = self.frame.last() {
                //         self.frame.push(current.scope());
                //     }
                // }
                _ => {
                    dbg!(&op);
                }
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

        // if self.memento.is_some() {
        //     return Ok(ValueKind::NONE);
        // }

        // let result = if !self.halt && self.memento.is_none() {
        //     if self.memory.len() == 0 {
        //         ValueKind::default()
        //     } else {
        //         *self.memory.pop().unwrap_or_default().kind()
        //     }
        // } else {
        //     ValueKind::NONE
        // };

        Ok(())
    }

    pub fn run(&mut self, code: Program<Code>) -> Result<ValueKind, Error> {
        self.memory.import_constants(code.get_constants());

        for (position, byte) in code.code().to_vec().iter().enumerate() {
            if byte.byte() == &Byte::Label {
                self.labels
                    .insert(byte.operand(0).copied().unwrap_or_default(), position);
            }
        }

        self.execute(code)?;

        Ok(self
            .memory
            .pop_value()
            .map(|v| *v.kind())
            .unwrap_or_default())
    }

    pub fn enter(&mut self, stack_offset: usize) {
        // enter!(self.frame, self.ip, stack_offset);
        self.frame
            .push(Frame::new(self.ip, self.memory.stack_size() - stack_offset));
    }

    pub fn leave(&mut self) {
        if let Some(c) = self.frame.last() {
            let val = self.memory.pop();

            self.ip = c.tell();
            self.memory.truncate(c.stack());

            if let Err(e) = self.memory.push(val) {
                eprintln!("ERR: {:?}", e);
            }

            if self.frame.pop().is_none() {
                self.halt = true;
            }
        } else {
            self.halt = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use common::program::Program;
    use common::{
        Value, ValueKind,
        interner::Interner,
        opcodes::{Byte, Code},
    };

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
