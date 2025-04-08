use common::likely;
use common::memory2::object::{ObjArray, ObjInstance, Objects};
use common::vec_array::VecArray;
use rustc_hash::{FxBuildHasher, FxHashMap as HashMap};
use std::io::{stderr, stdout};

use crate::frame::Frame;
use crate::options::MachineOptions;
use crate::utils::output::Output;
use common::memory2::{Heap, STACK_SIZE, Stack};
use common::program::program::Program;
use common::{
    Value,
    error::{Error, ErrorOrigin},
    opcodes::{Byte, Code},
};

const STACK_FRAMES: usize = 2048;
const STORAGE_CHUNKS: usize = 64;

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    ip: usize,
    fp: usize,
    stack_frames: [(usize, usize); STACK_FRAMES],
    variables: [VecArray<usize, STORAGE_CHUNKS>; STACK_FRAMES],
    stack: Stack<Value>,
    heap: Heap,
    // owner => name => label
    methods: HashMap<usize, HashMap<usize, usize>>,
    properties: HashMap<usize, Vec<usize>>,

    options: MachineOptions,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            halt: false,
            ip: 0,
            fp: 1,
            stack_frames: [(0, 0); STACK_FRAMES],
            variables: std::array::from_fn::<VecArray<usize, STORAGE_CHUNKS>, STACK_FRAMES, _>(
                |_| Default::default(),
            ),
            stack: Stack::new(),
            heap: Default::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
            options: MachineOptions::default(),
            methods: HashMap::default(),
            properties: HashMap::default(),
        }
    }
}

macro_rules! binary_op {
    ($rhs: expr, $lhs: expr, $op: tt) => {
            $lhs $op $rhs
    };
    ($rhs: expr, $lhs: expr, $op: tt, $kind: ident) => {{
        Value::$kind($lhs $op $rhs)
    }};
}

macro_rules! unary_op {
    ($rhs: expr, $op: tt) => {
            $op $rhs
    };
}

macro_rules! binary_handler {
    ($this: expr, $op: tt) => {{
        let rhs = $this.stack.pop();
        let lhs = $this.stack.pop();

        $this.stack.push(binary_op!(rhs, lhs, $op));
    }};
    ($this: expr, $op: tt, $type: ident) => {{
        let rhs = $this.stack.pop();
        let lhs = $this.stack.pop();

        $this.stack.push(binary_op!(rhs, lhs, $op, $type));
    }};
}

macro_rules! unary_handler {
    ($this: expr, $op: tt) => {{
        let rhs = $this.stack.pop();

        $this.stack.push(unary_op!(rhs, $op));
    }};
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
                Byte::Call => {
                    // likely(true);
                    if let Value::FUNCTION(arity, position) = self.stack.pop() {
                        self.enter(arity);
                        self.ip = position;
                    } else {
                        unreachable!("Attempting to call non-existing function");
                    }
                }
                Byte::Leave => {
                    self.leave();
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
                Byte::Store => {
                    // likely(true);
                    self.reassign(op.operand(0), self.stack.tell(1))
                }
                Byte::Load => {
                    let pos = self.lookup(op.operand(0));
                    self.stack.copy_to_top(pos);
                }
                Byte::Not => unary_handler!(self, !),
                Byte::Negate => unary_handler!(self, -),
                Byte::Less => binary_handler!(self, <, BOOLEAN),
                Byte::Greater => binary_handler!(self, >, BOOLEAN),
                Byte::Equal => binary_handler!(self, ==, BOOLEAN),
                Byte::Add => binary_handler!(self, +),
                Byte::Sub => binary_handler!(self, -),
                Byte::Mul => binary_handler!(self, *),
                Byte::Div => binary_handler!(self, /),
                Byte::Mod => binary_handler!(self, %),
                Byte::LShift => binary_handler!(self, <<),
                Byte::RShift => binary_handler!(self, >>),
                Byte::Xor => binary_handler!(self, ^),
                Byte::And => binary_handler!(self, &),
                Byte::Or => binary_handler!(self, |),
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
                Byte::Jump => self.ip = op.operand(0),
                Byte::Jumpr => self.ip += op.operand(0),
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

                    self.ip = if state { op.operand(0) } else { op.operand(1) };
                }
                Byte::Range => {
                    let last = self.stack.pop();
                    let first = self.stack.pop();

                    if let (Value::INTEGER(start), Value::INTEGER(end)) = (first, last) {
                        let items: Vec<Value> = (start..end).map(Value::from).collect();

                        self.gc();
                        let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);

                        self.stack.push(Value::OBJECT(obj_array));
                    } else {
                        unreachable!("Invalid start & end for range");
                    }
                }
                Byte::Array => {
                    // likely(true);
                    let len = op.operand(0);
                    let mut items = Vec::with_capacity(len);
                    items.append(&mut self.stack.npop(len).to_vec());

                    self.gc();
                    let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);
                    self.stack.push(Value::OBJECT(obj_array));
                }
                Byte::Instantiate => {
                    // likely(true);
                    let owner = op.operands()[0];
                    let mut object = ObjInstance::new(owner);
                    for (idx, field) in self.properties[&owner].iter().enumerate() {
                        if idx == 0 {
                            continue;
                        }
                        object.update(*field, Default::default());
                    }
                    self.gc();
                    let (instance, _) = self.heap.alloc(object, Objects::Object);

                    self.stack.push(Value::OBJECT(instance));
                    self.stack.push(Value::POINTER(self.stack.tell(1)));
                }
                Byte::Invoke => {
                    // likely(true);
                    let operands = op.operands();
                    let name = operands[0];
                    let arity = operands[1];

                    if let Value::OBJECT(Objects::Object(instance)) = self.peek_obj(arity) {
                        let owner = instance.as_ref().name();

                        self.enter(arity);
                        self.ip = self.methods[&owner][&name];
                    }
                }
                Byte::This => {
                    // likely(true);
                    let ptr = self.stack.peek(self.stack_frames[self.fp - 1].1 - 1);
                    self.stack.push(ptr);
                }
                Byte::Prop => {
                    // likely(true);
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
                Byte::Upvalue => {
                    let frame = op.operand(0);
                    let name = op.operand(1);
                    let upvalue = op.operand(2);

                    let val = self.lookup_upvalue(frame, upvalue);
                    self.reassign(name, val);
                }
                Byte::Label => {
                    self.ip += op.operand(1);
                }
                Byte::Halt => {
                    self.halt = true;
                }
                _ => (),
            }

            if self.halt || self.fp == 0 {
                break;
            }

            self.ip += 1;
        }

        Ok(())
    }

    pub fn run(&mut self, code: Program<Code>) -> Result<Value, Error> {
        // let bytes: Vec<(usize, Code)> = code.code().to_vec().iter().cloned().enumerate().collect();

        for byte in code.code() {
            match byte.byte() {
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

        let mut grey = Vec::with_capacity(32);
        self.mark_sweep(&mut grey);
    }

    // #[inline]
    fn mark_sweep(&mut self, grey_objects: &mut Vec<Objects>) {
        self.mark_roots(grey_objects);
        while let Some(object) = grey_objects.pop() {
            object.mark_references(grey_objects);
        }
        self.heap.sweep();
    }

    // #[inline]
    fn mark_roots(&mut self, grey_objects: &mut Vec<Objects>) {
        grey_objects.clear();
        for value in self.stack.iter() {
            match value {
                Value::OBJECT(mut o) => o.mark(grey_objects),
                Value::STRING(mut o) => o.mark(grey_objects),
                _ => (),
            }
        }
    }

    // #[inline]
    fn enter(&mut self, arity: usize) {
        self.stack_frames[self.fp] = (self.ip, self.stack.tell(arity));
        self.fp += 1;
        // self.variables.modify_or_insert(self.fp, |_| ());
    }

    // #[inline]
    fn leave(&mut self) {
        self.fp -= 1;

        let (ip, stack) = self.stack_frames[self.fp];

        self.ip = ip;
        self.stack.restore(stack);
    }

    // #[inline]
    fn lookup(&mut self, name: usize) -> usize {
        *self.variables[self.fp].get(name)
        // *self.variables.get(self.fp).get(name)
    }

    fn lookup_upvalue(&mut self, frame: usize, name: usize) -> usize {
        *self.variables[frame].get(name)
        // *self.variables.get(name)
    }

    // #[inline]
    fn reassign(&mut self, symbol: usize, position: usize) {
        self.variables[self.fp].insert(symbol, position);
    }

    // #[inline]
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
        constant.with_operands([num, 0, 0]);

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
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(0.9)));
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::INTEGER(8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(8.1)));
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::INTEGER(1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(program);
        assert!(result.is_ok());
        assert_eq!(result, Ok(Value::FLOAT(1.8)));
    }
}
