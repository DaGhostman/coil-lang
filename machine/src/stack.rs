use common::memory::object::{ObjArray, ObjInstance, Objects};
use rustc_hash::FxHashMap as HashMap;
use std::io::{stderr, stdout};

use crate::options::MachineOptions;
use crate::utils::output::Output;
use common::memory::{Heap, Stack};
use common::program::program::Program;
use common::{
    Value,
    opcodes::{Byte, Code},
};

const FRAMES: usize = 1024;
const STACK: usize = 8192;

pub struct Machine {
    stdout: Output,
    stderr: Output,
    halt: bool,

    ip: usize,
    fp: usize,
    // Figure out a way to not use
    call_stack: [(usize, usize); FRAMES],
    stack: Stack<Value, STACK>,
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
            call_stack: [(0, 0); FRAMES],
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

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, code: &Program<Code>) {
        self.ip = 0;
        while let Some(op) = code.get(self.ip) {
            #[cfg(feature = "trace")]
            {
                eprintln!(
                    "({:0>2})#{:0>8} {:?}\t{:?}",
                    self.fp,
                    self.ip,
                    op.byte(),
                    self.stack.iter().collect::<Vec<Value>>()
                );
            }
            #[cfg(feature = "trace")]
            {
                if self.fp > 32 {
                    break;
                }
            }
            match op.byte() {
                Byte::Call => {
                    if let Value::FUNCTION(arity, position) = self.stack.pop() {
                        if op.operand(1) == 1 {
                            self.fp = 0;
                        }

                        self.enter(arity);
                        self.ip = position;
                        continue;
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
                    self.stack.npop(op.operand(0));
                }
                Byte::Duplicate => {
                    self.stack.copy_to_top(self.stack.tell(1));
                }
                Byte::Store => self
                    .stack
                    .pop_to(self.call_stack[self.fp].1 + op.operand(0)),
                Byte::Load => {
                    self.stack
                        .copy_to_top(op.operand(0) + self.call_stack[self.fp - 1].1);
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
                    self.stdout.write(&match self.stack.pop() {
                        Value::STR(idx) => code.data().string(idx).to_string(),
                        value => value.to_string(),
                    });

                    if op.operand(0) == 1 {
                        self.stdout.write("\n");
                    }
                }
                Byte::Jump => {
                    self.ip = op.operand(0);
                    continue;
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
                        _ => true,
                    };

                    if !state {
                        self.ip = op.operand(0);
                        continue;
                    }
                }
                Byte::Range => {
                    let last = self.stack.pop();
                    let first = self.stack.pop();

                    if let (Value::INTEGER(start), Value::INTEGER(end)) = (first, last) {
                        let items: Vec<Value> = (start..end).map(Value::from).collect();

                        self.gc();
                        let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);

                        self.stack.push(Value::OBJECT(obj_array));
                    }
                }
                Byte::Array => {
                    // likely(true);
                    let len = op.operand(0);
                    let mut items = Vec::with_capacity(len);
                    items.copy_from_slice(self.stack.npop(len));

                    self.gc();
                    let (obj_array, _) = self.heap.alloc(ObjArray::from(items), Objects::Array);
                    self.stack.push(Value::OBJECT(obj_array));
                }
                Byte::Iterate => {
                    let iter = self.stack.pop();
                    let arr = self.stack.peek(self.stack.tell(1));
                    if let (Value::ITERATOR(n), Value::OBJECT(Objects::Array(arr))) = (iter, arr) {
                        if n >= arr.as_ref().len() {
                            self.ip = op.operand(0);
                            continue;
                        }

                        self.stack.push(Value::ITERATOR(n + 1));
                        self.stack.pop_to(op.operand(1));
                        self.stack.push(*arr.as_ref().item(n));
                    } else {
                        todo!(
                            "Handle cases where value is not an array, but object that actually implements iterator interface or error if it is not a valid iterable"
                        );
                    }
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
                    self.stack.push(Value::REFERENCE(self.stack.tell(1)));
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
                        continue;
                    }
                }
                Byte::This => {
                    // likely(true);
                    let ptr = self.stack.peek(self.call_stack[self.fp - 1].1 - 1);
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
                            _ => (),
                        }
                    }
                }
                Byte::Upvalue => {
                    let frame = op.operand(0);
                    let upvalue = op.operand(2);

                    self.stack.copy_to_top(self.call_stack[frame].1 + upvalue);
                }
                Byte::Halt => {
                    self.halt = true;
                }
                _ => (),
            }

            #[cfg(feature = "stress")]
            {
                self.gc()
            }

            if self.halt || self.fp == 0 {
                break;
            }

            self.ip += 1;
        }
    }

    pub fn run(&mut self, code: &Program<Code>) -> Value {
        for byte in code.code() {
            if byte.byte() == &Byte::Method {
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
        }

        self.execute(code);

        self.stack.pop()
    }

    fn gc(&mut self) {
        #[cfg(not(feature = "stress"))]
        if self.heap.size() <= self.heap.threshold() {
            return;
        }

        self.mark_sweep();
    }

    // #[inline]
    fn mark_sweep(&mut self) {
        let mut grey_objects = Vec::with_capacity(32);
        self.mark_roots(&mut grey_objects);
        while let Some(object) = grey_objects.pop() {
            object.mark_references(&mut grey_objects);
        }

        self.heap.sweep();
    }

    // #[inline]
    fn mark_roots(&mut self, grey_objects: &mut Vec<Objects>) {
        grey_objects.clear();
        for value in &self.stack {
            match value {
                Value::OBJECT(mut o) | Value::STRING(mut o) => o.mark(grey_objects),
                _ => (),
            }
        }
    }

    // #[inline]
    fn enter(&mut self, arity: usize) {
        self.call_stack[self.fp] = (self.ip, self.stack.tell(arity));
        self.fp += 1;
    }

    // #[inline]
    fn leave(&mut self) {
        self.fp -= 1;

        // let (ip, stack) = self.call_stack.get(self.fp);
        let (ip, stack) = self.call_stack[self.fp];

        self.ip = ip;
        self.stack.restore(stack);
    }

    // // #[inline]
    // fn lookup(&mut self, name: usize) -> usize {
    //     *self.variables[self.fp].get(name)
    // }
    //
    // fn lookup_upvalue(&mut self, frame: usize, name: usize) -> usize {
    //     *self.variables[frame].get(name)
    // }
    //
    // // #[inline]
    // fn reassign(&mut self, symbol: usize, position: usize) {
    //     self.variables[self.fp].insert(symbol, position);
    // }

    // #[inline]
    fn peek_obj(&self, position: usize) -> Value {
        match self.stack.peek(self.stack.tell(position)) {
            Value::REFERENCE(n) => self.stack.peek(n),
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

        let mut program = Program::new(vec![constant, constant, Code::new(Byte::Add)]);
        program.with_data(values);
        let result = Machine::default().run(&program);

        assert_eq!(result, Value::INTEGER(4));
    }

    #[test]
    fn test_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(&program);
        assert_eq!(result, Value::FLOAT(0.9));
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::INTEGER(8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(&program);
        assert_eq!(result, Value::FLOAT(8.1));
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(Byte::Push, [values.add_constant(Value::FLOAT(0.8)), 0, 0]);
        let b = Code::new_with_operands(Byte::Push, [values.add_constant(Value::INTEGER(1)), 0, 0]);

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);
        program.with_data(values);

        let result = Machine::default().run(&program);
        assert_eq!(result, Value::FLOAT(1.8));
    }
}
