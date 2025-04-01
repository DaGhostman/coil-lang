use common::memory2::object::{ObjArray, ObjInstance, Objects};
use rustc_hash::FxHashMap as HashMap;
use std::io::{stderr, stdout};

use crate::frame::Frame;
use crate::options::MachineOptions;
use crate::utils::output::Output;
use common::memory2::{Heap, Stack};
use common::program::program::Program;
use common::{
    Value,
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
};

const FRAME_SIZE: usize = 64;
const MAX_LABELS: usize = u16::MAX as usize;

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    ip: usize,
    fp: usize,
    frames: Vec<Frame>,
    stack: Stack<Value>,
    heap: Heap,
    labels: [usize; MAX_LABELS],
    // owner => name => label
    methods: HashMap<usize, HashMap<usize, usize>>,
    properties: HashMap<usize, Vec<usize>>,

    options: MachineOptions,

    grey_objects: Vec<Objects>,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            ip: 0,
            fp: 0,
            stack: Stack::new(),
            frames: vec![Default::default(); FRAME_SIZE],
            heap: Heap::new(2, 1024 * 1024),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
            options: MachineOptions::default(),
            labels: [0; MAX_LABELS],
            methods: HashMap::default(),
            properties: HashMap::default(),
            grey_objects: Vec::default(),
        }
    }
}

macro_rules! binary_op {
    ($rhs: expr, $lhs: expr, $op: tt) => {
        if let Some(result) = ($lhs $op $rhs) {
            result
        } else {
            return Err(Error::new(
                ErrorOrigin::RUNTIME,
                String::from("Operands do not match any valid types"),
            ));
        }
    };
    ($rhs: expr, $lhs: expr, $op: tt, $kind: ident) => {
        Value::$kind($lhs $op $rhs)
    };
}

macro_rules! unary_op {
    ($rhs: expr, $op: tt) => {
        if let Some(result) = $op $rhs {
            result
        } else {
            return Err(Error::new(
                ErrorOrigin::RUNTIME,
                String::from("Operand do not match any valid types"),
            ));
        }
    };
}

impl Machine {
    pub fn with_options(options: MachineOptions) -> Self {
        let mut this = Self::default();
        this.options = options;
        this.stdout = Output::new(&this.options, || Box::new(stdout().lock()));
        this.stderr = Output::new(&this.options, || Box::new(stderr().lock()));
        this.heap = Heap::new(this.options.gc_growth(), this.options.gc_threshold());
        this.stack = Stack::new();

        this
    }

    fn execute(&mut self, code: &Program<Code>) -> Result<(), Error> {
        self.ip = 0;
        while let Some(op) = code.get(self.ip) {
            // eprintln!(
            //     "({:0>2})#{:0>8} {:?}\t{:?}",
            //     self.fp,
            //     self.ip,
            //     op.byte(),
            //     self.stack.iter().collect::<Vec<Value>>()
            // );
            match op.byte() {
                Byte::Label => {
                    self.ip += op.operand(1);
                }
                Byte::Call => {
                    if let Value::FUNCTION(arity, label) = self.stack.pop() {
                        self.enter(arity);
                        self.ip = self.labels[label];
                    } else {
                        unreachable!("Attempting to call non-existing function");
                    }
                }
                Byte::Halt => {
                    self.halt = true;
                }
                Byte::Enter => self.enter(0),
                Byte::Leave => {
                    if self.fp == 0 {
                        self.halt = true;
                    } else {
                        self.fp -= 1;
                    }
                    let frame = &self.frames[self.fp];

                    self.ip = frame.tell();
                    self.stack.restore(frame.stack());
                    if self.heap.has_allocated() {
                        self.gc();
                    }

                    // self.leave()?
                }
                Byte::Push => {
                    let constant = code.data().constant(op.operand(0));
                    self.stack.push(*constant);
                }
                Byte::Pop => {
                    self.stack.pop();
                }
                Byte::Peek => {
                    self.reassign(op.operand(0), (self.stack.len() - 1) - op.operand(1));
                }
                Byte::Upvalue => {
                    let frame = op.operand(0);
                    let name = op.operand(1);
                    let upvalue = op.operand(2);

                    self.reassign(name, self.lookup_upvalue(frame, upvalue));
                }
                Byte::Store => self.reassign(op.operand(0), self.stack.tell(1)),
                Byte::Load => {
                    self.stack.copy_to_top(self.lookup(op.operand(0)));
                }
                Byte::Not => {
                    let rhs = self.stack.pop();
                    self.stack.push(unary_op!(rhs, !))
                }
                Byte::Negate => {
                    let rhs = self.stack.pop();
                    self.stack.push(unary_op!(rhs, -))
                }
                Byte::Less => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, <, BOOLEAN))
                }
                Byte::Greater => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, >, BOOLEAN))
                }
                Byte::Add => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, +))
                }
                Byte::Sub => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, -))
                }
                Byte::Mul => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, *))
                }
                Byte::Div => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, /))
                }
                Byte::Mod => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, %))
                }
                Byte::LShift => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, <<))
                }
                Byte::RShift => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, >>))
                }
                Byte::Xor => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, ^))
                }
                Byte::And => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, &))
                }
                Byte::Or => {
                    let rhs = self.stack.pop();
                    let lhs = self.stack.pop();

                    self.stack.push(binary_op!(rhs, lhs, |))
                }
                Byte::Print => {
                    self.stdout.write(
                        (match self.stack.pop() {
                            Value::STR(idx) => code.data().string(idx).to_string(),
                            value => value.to_string(),
                        })
                        .to_string(),
                    );

                    if op.operand(0) == 1 {
                        self.stdout.write("\n".to_string());
                    }
                }
                Byte::Jump => {
                    self.ip = self.labels[op.operand(0)];
                }
                Byte::Jumpz => {
                    let value = self.stack.pop();
                    if !matches!(value, Value::BOOLEAN(_)) {
                        eprintln!("Condition does not evaluate to boolean, using fallback checks");
                    }

                    let state = match value {
                        Value::NONE => false,
                        Value::BOOLEAN(byte) => byte,
                        Value::FLOAT(value) => value != 0.0,
                        Value::INTEGER(value) => value != 0,
                        _ => panic!("Unable to evaluate"),
                    };

                    self.ip = self.labels[if state { op.operand(0) } else { op.operand(1) }];
                }
                Byte::Range => {
                    let last = self.stack.pop();
                    let first = self.stack.pop();

                    if let (Value::INTEGER(start), Value::INTEGER(end)) = (first, last) {
                        let items: Vec<Value> = (start..end).map(Value::from).collect();

                        let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);

                        self.stack.push(Value::OBJECT(obj_array));
                    } else {
                        unreachable!("Invalid start & end for range");
                    }
                }
                Byte::Array => {
                    let len = op.operand(0);
                    let mut items = Vec::with_capacity(len);
                    items.append(&mut self.stack.npop(len).to_vec());

                    let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);
                    self.stack.push(Value::OBJECT(obj_array));
                }
                Byte::Instantiate => {
                    let owner = op.operands()[0];
                    let mut object = ObjInstance::new(owner);
                    for (idx, field) in self.properties[&owner].iter().enumerate() {
                        if idx == 0 {
                            continue;
                        }
                        object.update(*field, Default::default());
                    }
                    let (instance, _) = self.heap.alloc(object, Objects::Object);

                    self.stack.push(Value::OBJECT(instance));
                    self.stack.push(Value::POINTER(self.stack.tell(1)));
                }
                Byte::Invoke => {
                    let operands = op.operands();
                    let name = operands[0];
                    let arity = operands[1];

                    if let Value::OBJECT(Objects::Object(instance)) = self.peek_obj(arity) {
                        let owner = instance.as_ref().name();

                        self.enter(arity);
                        self.ip = self.labels[self.methods[&owner][&name]];
                    }
                }
                Byte::This => {
                    let ptr = self.stack.peek(self.frames[self.fp - 1].stack() - 1);
                    self.stack.push(ptr);
                }
                Byte::Prop => {
                    let operands = op.operands();
                    if let Value::OBJECT(Objects::Object(mut obj)) = self.peek_obj(0) {
                        match operands[1] {
                            0 => {
                                self.stack.pop();
                                if let Some(value) = obj.as_ref().get(operands[0]) {
                                    self.stack.push(*value);
                                }
                            }
                            1 => {
                                obj.as_mut().update(operands[0], self.stack.pop());
                                self.stack.pop();
                            }
                            2 => {
                                let [owner, _, field, ..] = op.operands();
                                self.properties
                                    .entry(*owner)
                                    .and_modify(|state| {
                                        state.push(*field);
                                    })
                                    .or_insert_with(|| vec![*field]);
                            }
                            _ => unreachable!("Unable to fetch state field"),
                        }
                    }
                }
                Byte::Method => (),
                op => {
                    return Err(Error::new(
                        ErrorOrigin::RUNTIME,
                        format!("Missing/unimplemented instruction {:?}", op),
                    ));
                }
            }

            if self.halt {
                break;
            }

            self.ip += 1;
        }

        Ok(())
    }

    pub fn run(&mut self, code: Program<Code>) -> Result<Value, Error> {
        let bytes: Vec<(usize, Code)> = code.code().to_vec().iter().cloned().enumerate().collect();

        for (position, byte) in bytes {
            match byte.byte() {
                Byte::Label => {
                    self.labels[byte.operand(0)] = position;
                }
                Byte::Method => {
                    let operands = byte.operands();

                    self.methods
                        .entry(operands[0])
                        .and_modify(|entry| {
                            entry.insert(operands[1], operands[2]);
                        })
                        .or_insert_with(|| {
                            let mut fields = HashMap::default();
                            fields.insert(operands[1], operands[2]);

                            fields
                        });
                }
                _ => (),
            }
        }

        self.execute(&code)?;

        Ok(self.stack.pop())
    }

    fn gc(&mut self) {
        if self.heap.size() <= self.heap.threshold() {
            return;
        }

        self.mark_sweep();
    }

    fn mark_sweep(&mut self) {
        self.mark_roots();
        while let Some(object) = self.grey_objects.pop() {
            object.mark_references(&mut self.grey_objects);
        }
        self.heap.sweep();
    }

    fn mark_roots(&mut self) {
        self.grey_objects.clear();
        for value in self.stack.iter() {
            match value {
                Value::OBJECT(mut o) => o.mark(&mut self.grey_objects),
                Value::STRING(mut o) => o.mark(&mut self.grey_objects),
                _ => (),
            }
        }
    }

    fn enter(&mut self, arity: usize) {
        let length = self.frames.len();
        if length < self.fp {
            self.frames.resize(length + FRAME_SIZE, Default::default());
        }

        self.frames[self.fp].replace(self.ip, self.stack.tell(arity));
        self.fp += 1;
    }

    fn leave(&mut self) -> Result<(), Error> {
        if self.fp == 0 {
            self.halt = true;
        } else {
            self.fp -= 1;
        }
        let frame = &self.frames[self.fp];

        self.ip = frame.tell();
        self.stack.restore(frame.stack());
        if self.heap.has_allocated() {
            self.gc();
        }

        Ok(())
    }

    fn lookup(&self, name: usize) -> usize {
        self.frames[self.fp].lookup(name)
    }

    fn lookup_upvalue(&self, frame: usize, name: usize) -> usize {
        self.frames[frame].lookup(name)
    }

    fn reassign(&mut self, symbol: usize, position: usize) {
        self.frames[self.fp].overwrite(symbol, position);
    }

    fn peek_obj(&self, position: usize) -> Value {
        match self.stack.peek(self.stack.tell(position + 1)) {
            Value::POINTER(n) => self.stack.peek(n),
            v => {
                dbg!(&v);
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use common::program::data::Data;
    use common::program::program::Program;
    use common::{
        Value,
        opcodes::{Byte, Code},
    };

    use crate::stack::Machine;

    #[test]
    fn test_integer_addition() {
        let mut values = Data::default();
        let num = values.add_constant(Value::INTEGER(2));
        let mut constant = Code::new(Byte::Push);
        constant.with_operands([num, 0, 0, 0, 0]);

        let mut program = Program::new(vec![
            constant.clone(),
            constant.clone(),
            Code::new(Byte::Add),
        ]);
        program.with_data(values);
        let result = Machine::default().run(program);

        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::INTEGER(4)));
    }

    #[test]
    fn test_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::FLOAT(0.8))]);
        let b = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::FLOAT(0.1))]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(0.9)));
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::INTEGER(8))]);
        let b = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::FLOAT(0.1))]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(8.1)));
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::FLOAT(0.8))]);
        let b = Code::new_with_operands(Byte::Push, vec![values.add_constant(Value::INTEGER(1))]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(1.8)));
    }
}
