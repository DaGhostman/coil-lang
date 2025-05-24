use common::likely;
use common::memory::collector::{Collectable, GcSized};
use common::memory::object::{
    ObjArray, ObjCoroutine, ObjInstance, ObjIterator, ObjString, Objects,
};
use common::native::{Action, Native};
use common::program::data::Data;
use common::types::Type;
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

const FRAMES: usize = 8192;
const STACK: usize = i16::MAX as usize; // 16365;

pub type ExternalFunction = dyn Fn(usize, &[Value], &mut Machine);

pub struct Machine {
    stdout: Output,
    stderr: Output,

    ip: usize,
    fp: usize,
    frames: [(usize, usize); FRAMES],
    stack: Stack<Value, STACK>,
    native: HashMap<usize, Native>,
    heap: Heap,
    options: MachineOptions,
}
impl Default for Machine {
    fn default() -> Self {
        Self {
            ip: 0,
            fp: 1,
            frames: [(0, 0); FRAMES],
            native: HashMap::default(),
            stack: Stack::new(),
            heap: Heap::default(),
            stdout: Output::new(&MachineOptions::default(), || Box::new(stdout().lock())),
            stderr: Output::new(&MachineOptions::default(), || Box::new(stderr().lock())),
            options: MachineOptions::default(),
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
        let rhs = *$this.stack.pop();
        let lhs = *$this.stack.pop();
        $this.stack.push(binary_op!(&rhs, &lhs, $op));
    }};
    ($this: expr, $op: tt, $type: ident) => {{
        let rhs = *$this.stack.pop();
        let lhs = *$this.stack.pop();
        $this.stack.push(binary_op!(&rhs, &lhs, $op, $type));
    }};
}

macro_rules! unary_handler {
    ($this: expr, $op: tt) => {{
        let rhs = $this.stack.pop();
        let result = unary_op!(rhs, $op);

        $this.stack.push(result);
    }};
}

macro_rules! validate {
    ($assert: expr, $pattern: pat, $msg: expr) => {
        #[cfg(not(debug_assertions))]
        {
            common::guarantee!(matches!($assert, $pattern))
        }
        #[cfg(debug_assertions)]
        {
            debug_assert!(matches!($assert, $pattern), $msg)
        }
    };
    ($assert: expr, $pattern: pat) => {
        #[cfg(not(debug_assertions))]
        {
            common::guarantee!(matches!($assert, $pattern))
        }
        #[cfg(debug_assertions)]
        {
            debug_assert!(matches!($assert, $pattern))
        }
    };
}

impl Machine {
    #[must_use]
    pub fn with_options(options: MachineOptions) -> Self {
        let mut this = Self::default();
        this.options = options;
        this.stdout = Output::new(&this.options, || Box::new(stdout().lock()));
        this.stderr = Output::new(&this.options, || Box::new(stderr().lock()));
        this.heap = Heap::new(this.options.gc_growth(), this.options.gc_threshold());
        this.stack = Stack::new();

        this
    }

    pub fn register(&mut self, library: Native) {
        self.native.insert(library.get_name(), library);
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, code: &Program<Code>, data: &Data) {
        self.ip = 0;

        loop {
            let op = code.get(self.ip);
            let operands = op.operands();

            #[cfg(feature = "trace")]
            {
                eprintln!(
                    "({:0>2})#{:0>8} {:?}\t{:?}",
                    self.fp,
                    self.ip,
                    op.byte(),
                    self.stack.iter().collect::<Vec<&Value>>()
                );

                // if self.fp > 32 || self.stack.len() > 16 {
                //     break;
                // }
            }

            match &op.byte() {
                Byte::Native => {
                    let &[arity, ..] = operands;
                    validate!(
                        self.stack.peek(0),
                        Value::NATIVE(..),
                        "Native function call should receive external references"
                    );

                    if let &Value::NATIVE(func) = self.stack.pop() {
                        let native = self.native[&func];
                        let args = self.stack.npop(arity);

                        // dbg!(
                        //     &self
                        //         .native
                        //         .iter()
                        //         .map(|(_, n)| n
                        //             .get_functions(&mut data.clone())
                        //             .iter()
                        //             .map(|(n, _)| *n)
                        //             .collect::<Vec<_>>())
                        //         .collect::<Vec<_>>()
                        // );

                        match
                            native.call(&args, &data)
                            // lib.call(data.symbol_name(func).as_str(), data, args)
                        {
                            Action::Push(value) => {
                                self.stack.push(value);
                            }
                            Action::Resume(position, stack, value) => {
                                self.enter(0);
                                self.ip = position;
                                for val in stack {
                                    self.stack.push(val);
                                }
                                self.stack.push(value);
                            }
                            Action::Fail(err) => {
                                eprintln!("Runtime Error: {err}");
                                break;
                            }
                            Action::Allocate(func, args) => {
                                let obj = func(&mut self.heap, &data, args.unwrap_or_default());

                                self.stack.push(obj);
                            }
                            _ => (),
                        }
                    }
                }
                Byte::Call => {
                    validate!(
                        self.stack.peek(0),
                        Value::FUNCTION(..),
                        "Calling a function, should receive a function to call"
                    );

                    if let &Value::FUNCTION(arity, position) = self.stack.pop() {
                        self.enter(arity);
                        self.ip = position;
                    }
                }
                Byte::Leave => self.leave(),
                Byte::Push => {
                    let &[value, ..] = operands;
                    let constant = data.constant(value);

                    self.stack.push(*constant);
                }
                Byte::Pop => {
                    let &[n, ..] = operands;
                    self.stack.rewind_by(n);
                }
                Byte::Store => {
                    let &[offset, ..] = operands;
                    let value = self.stack.peek(0);

                    self.stack.set(self.frame_add(offset), *value);
                }
                Byte::Load => {
                    let &[offset, ..] = operands;

                    dbg!(offset);
                    self.stack.push(*self.stack.get(self.frame_add(offset)));
                }
                Byte::Not => unary_handler!(self, !),
                Byte::Negate => unary_handler!(self, -),
                Byte::Less => binary_handler!(self, <, BOOLEAN),
                Byte::LessEqual => binary_handler!(self, <=, BOOLEAN),
                Byte::Greater => binary_handler!(self, >, BOOLEAN),
                Byte::GreaterEqual => binary_handler!(self, >=, BOOLEAN),
                Byte::Equal => binary_handler!(self, ==, BOOLEAN),
                Byte::Yield => {
                    let &result = self.stack.pop();
                    let (obj, mut coro) = self.alloc(ObjCoroutine::default(), Objects::Coroutine);
                    let stack = self.stack.npop(self.stack.tell(self.frame()));

                    let ip = self.ip;
                    coro.as_mut().suspend(ip, stack.to_vec());
                    coro.as_mut().set(result);

                    self.stack.push(Value::OBJECT(obj));
                    self.leave();
                }
                Byte::Add => match (self.stack.peek(1), self.stack.peek(0)) {
                    (
                        &Value::OBJECT(Objects::String(lhs)),
                        &Value::OBJECT(Objects::String(rhs)),
                    ) => {
                        if rhs.as_ref().len() == 0 {
                            self.stack.pop();
                        } else if lhs.as_ref().len() == 0 {
                            let l = *self.stack.pop();
                            self.stack.pop();

                            self.stack.push(l);
                        } else {
                            let (obj_string, _) = self.alloc(
                                ObjString::from(format!("{}{}", lhs.as_ref(), rhs.as_ref())),
                                Objects::String,
                            );

                            self.stack.npop(2);
                            self.stack.push(Value::OBJECT(obj_string));
                        }
                    }
                    (Value::OBJECT(Objects::String(lhs)), Value::STR(idx)) => {
                        let rhs = data.string(*idx);
                        let (obj_string, _) = self.alloc(
                            ObjString::from(format!("{}{}", lhs.as_ref(), rhs)),
                            Objects::String,
                        );

                        self.stack.npop(2);
                        self.stack.push(Value::OBJECT(obj_string));
                    }
                    (Value::STR(idx), Value::OBJECT(Objects::String(rhs))) => {
                        let lhs = data.string(*idx);
                        let (obj_string, _) = self.alloc(
                            ObjString::from(format!("{}{}", lhs, rhs.as_ref())),
                            Objects::String,
                        );

                        self.stack.npop(2);
                        self.stack.push(Value::OBJECT(obj_string));
                    }
                    (Value::STR(lhs), Value::STR(rhs)) => {
                        let lhs = data.string(*lhs);
                        let rhs = data.string(*rhs);

                        let (obj_string, _) = self
                            .heap
                            .alloc(ObjString::from(format!("{lhs}{rhs}")), Objects::String);

                        self.stack.npop(2);
                        self.stack.push(Value::OBJECT(obj_string));
                    }
                    (Value::STR(lhs), rhs) => {
                        let lhs = data.string(*lhs);

                        let (obj_string, _) = self
                            .heap
                            .alloc(ObjString::from(format!("{lhs}{rhs}")), Objects::String);

                        self.stack.npop(2);
                        self.stack.push(Value::OBJECT(obj_string));
                    }
                    _ => binary_handler!(self, +),
                },
                Byte::Sub => binary_handler!(self, -),
                Byte::Mul => binary_handler!(self, *),
                Byte::Div => binary_handler!(self, /),
                Byte::Mod => binary_handler!(self, %),
                Byte::Pow => {
                    let &rhs = self.stack.pop();
                    let &lhs = self.stack.pop();

                    match (lhs, rhs) {
                        (Value::INTEGER(lhs), Value::INTEGER(rhs)) => {
                            self.stack
                                .push(Value::INTEGER(lhs.pow(rhs.try_into().unwrap_or_default())));
                        }
                        (Value::FLOAT(lhs), Value::INTEGER(rhs)) => {
                            self.stack.push(Value::FLOAT(lhs.powf(rhs as f64)));
                        }
                        (Value::INTEGER(lhs), Value::FLOAT(rhs)) => {
                            self.stack.push(Value::INTEGER(
                                lhs.pow((rhs as i64).try_into().unwrap_or_default()),
                            ));
                        }
                        (Value::FLOAT(lhs), Value::FLOAT(rhs)) => {
                            self.stack.push(Value::FLOAT(lhs.powf(rhs)));
                        }
                        _ => (),
                    }
                }
                Byte::LShift => binary_handler!(self, <<),
                Byte::RShift => binary_handler!(self, >>),
                Byte::Xor => binary_handler!(self, ^),
                Byte::And => binary_handler!(self, &),
                Byte::Or => binary_handler!(self, |),
                Byte::Print => {
                    let &[newline, ..] = operands;

                    self.stdout.write(&match self.stack.peek(0) {
                        Value::STR(idx) => data.string(*idx).to_string(),
                        Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                        Value::TYPE(ty) => data.get_type(*ty).output(data),
                        Value::OBJECT(Objects::Array(arr)) => {
                            let length = arr.as_ref().len();

                            let items: Vec<String> = arr
                                .as_ref()
                                .clone()
                                .into_iter()
                                .take(2)
                                .map(|v| v.to_string())
                                .collect();

                            format!(
                                "[{}{}]",
                                items.join(", "),
                                if length > 2 {
                                    format!(", .., {}", arr.as_ref().item(length - 1))
                                } else {
                                    String::new()
                                }
                            )
                        }

                        value => value.to_string(),
                    });
                    self.stack.pop();

                    if newline == 1 {
                        self.stdout.write("\n");
                    }
                }
                Byte::TypeOf => {
                    let result = Value::TYPE(data.find_type(Type::new(self.stack.pop().into())));

                    self.stack.push(result);
                }
                Byte::Jumpz => {
                    let &[ip, ..] = operands;
                    let value = self.stack.pop();

                    if likely(
                        matches!(value, Value::NONE) || matches!(value, Value::BOOLEAN(false)),
                    ) {
                        self.ip = ip;
                    }
                }
                Byte::Jump => {
                    let &[ip, ..] = operands;
                    self.ip = ip;
                }
                Byte::Range => {
                    let &[inclusive, ..] = operands;
                    validate!(
                        self.stack.peek(0),
                        Value::INTEGER(..),
                        "Expected start index to be an integer"
                    );
                    validate!(
                        self.stack.peek(1),
                        Value::INTEGER(..),
                        "Expected end index to be an integer"
                    );

                    if let &[Value::INTEGER(start), Value::INTEGER(end)] = self.stack.npop(2) {
                        let items: Vec<Value> = if inclusive == 1 {
                            (start..=end).map(Value::from).collect()
                        } else {
                            (start..end).map(Value::from).collect()
                        };

                        let (obj_array, _) = self.alloc(ObjArray::from(items), Objects::Array);

                        self.stack.push(Value::OBJECT(obj_array));
                    }
                }
                Byte::Array => {
                    let &[len, ..] = operands;
                    let mut items = Vec::with_capacity(len);
                    items.copy_from_slice(self.stack.npop(len));

                    let (obj_array, _) = self.alloc(ObjArray::from(items), Objects::Array);
                    self.stack.push(Value::OBJECT(obj_array));
                }
                Byte::Iterator => {
                    let &[val, iterator, ..] = operands;

                    let iter = *self.stack.peek(0);

                    validate!(
                        iter,
                        Value::OBJECT(Objects::Array(..)),
                        "Iterators should be an array"
                    );

                    if let Value::OBJECT(Objects::Array(arr)) = iter {
                        let (iter, _) = self.alloc(
                            ObjIterator::new(Value::OBJECT(Objects::Array(arr))),
                            Objects::Iterator,
                        );

                        self.stack
                            .set(self.frame_add(iterator), Value::OBJECT(iter));
                        self.stack.set(self.frame_add(val), arr.as_ref().item(0));
                    }
                }
                Byte::Iterate => {
                    let &[position, var, iterator] = operands;
                    let arr = self.stack.get(self.frame_add(iterator));
                    validate!(
                        arr,
                        Value::OBJECT(Objects::Iterator(..)),
                        "Iterations expect to receive iterator"
                    );

                    if let Value::OBJECT(Objects::Iterator(mut obj)) = *arr {
                        if !likely(obj.as_ref().valid()) {
                            self.ip = position;
                        } else {
                            let value = obj.as_ref().get();
                            obj.as_mut().next();

                            self.stack.set(var, value);
                        }
                    }
                }
                Byte::Instantiate => {
                    let &[owner, arity, _] = operands;
                    let object = ObjInstance::new(owner);
                    let (instance, o) = self.alloc(object, Objects::Object);

                    dbg!((o.as_ref(), arity, self.stack.tell(arity)));

                    self.stack.push(Value::OBJECT(instance));
                }
                Byte::This => {
                    let idx = self.frame_sub(1);
                    self.stack.push(*self.stack.get(idx));
                }
                Byte::Prop => {
                    let &[name, action, ..] = operands;

                    match action {
                        0 => {
                            let val = if let Value::OBJECT(Objects::Object(object)) =
                                self.stack.peek(0)
                            {
                                if let Some(value) = object.as_ref().get(name) {
                                    *value
                                } else {
                                    Value::default()
                                }
                            } else {
                                Value::default()
                            };

                            self.stack.pop();
                            self.stack.push(val);
                        }
                        1 => {
                            dbg!(self.stack.peek(self.frame()), self.frame());
                            if let &Value::OBJECT(Objects::Object(mut object)) =
                                self.stack.peek(self.frame())
                            {
                                if let [a, val] = self.stack.npop(2) {
                                    dbg!(&a, val);
                                    object.as_mut().update(name, val);
                                }
                            }
                        }
                        _ => (),
                    }
                }
                Byte::Upvalue => {
                    let &[.., upvalue] = operands;
                    let value = self.stack.get(upvalue);
                    self.stack.push(*value);
                }
                Byte::Length => {
                    let length = match *self.stack.pop() {
                        Value::STR(str) => data.string(str).len(),
                        Value::OBJECT(Objects::String(str)) => str.as_ref().len(),
                        Value::OBJECT(Objects::Object(obj)) => obj.as_ref().all().len(),
                        Value::OBJECT(Objects::Array(arr)) => arr.as_ref().len(),
                        _ => 0,
                    };

                    self.stack.push(Value::INTEGER(length as i64));
                }
                Byte::Halt => {
                    break;
                }
                _ => (),
            }

            #[cfg(feature = "stress")]
            {
                self.gc();
            }

            self.ip += 1;
        }
    }

    pub fn run(&mut self, code: &Program<Code>, data: &Data) {
        self.execute(code, data);
    }

    fn alloc<T: GcSized, F: Fn(Collectable<T>) -> Objects>(
        &mut self,
        value: T,
        map: F,
    ) -> (Objects, Collectable<T>) {
        self.gc();

        self.heap.alloc(value, map)
    }

    fn gc(&mut self) {
        #[cfg(not(feature = "stress"))]
        if self.heap.size() <= self.heap.threshold() {
            return;
        }

        self.mark_sweep();
    }

    fn mark_sweep(&mut self) {
        let mut grey_objects = Vec::with_capacity(32);
        self.mark_roots(&mut grey_objects);
        while let Some(object) = grey_objects.pop() {
            object.mark_references(&mut grey_objects);
        }

        self.heap.sweep();
    }

    fn mark_roots(&mut self, grey_objects: &mut Vec<Objects>) {
        grey_objects.clear();
        for value in self.stack.iter_mut() {
            match value {
                Value::OBJECT(o) => o.mark(grey_objects),
                _ => (),
            }
        }
    }

    fn enter(&mut self, arity: usize) {
        debug_assert!(self.fp < FRAMES);

        self.frames[self.fp] = (self.ip, self.stack.tell(arity));
        self.fp += 1;
    }

    fn leave(&mut self) {
        debug_assert!(self.fp > 0 && self.fp < FRAMES);

        self.fp -= 1;
        let (ip, stack) = self.frames[self.fp];

        self.ip = ip;
        self.stack.restore(stack);
    }

    #[inline]
    fn frame(&self) -> usize {
        self.frames[self.fp - 1].1
    }

    fn frame_add(&self, offset: usize) -> usize {
        debug_assert!(self.frame() <= FRAMES);

        self.frame() + offset
    }

    fn frame_sub(&self, offset: usize) -> usize {
        debug_assert!(self.frame() >= offset);

        self.frame() - offset
    }
}

#[cfg(test)]
mod tests {
    use common::program::data::Data;
    use common::program::program::Program;
    use common::types::Type;
    use common::{
        Value,
        opcodes::{Byte, Code},
    };

    use crate::stack::Machine;

    #[test]
    fn test_integer_addition() {
        let mut values = Data::default();
        let num = values.add_constant(Value::INTEGER(2), Type::integer());
        let mut constant = Code::new(Byte::Push);
        constant.with_operands([num, 0, 0]);

        let mut program = Program::new(vec![constant, constant, Code::new(Byte::Add)]);
        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.8), Type::float()), 0, 0],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.1), Type::float()), 0, 0],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_int_float_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [
                values.add_constant(Value::INTEGER(8), Type::integer()),
                0,
                0,
            ],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.1), Type::float()), 0, 0],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }

    #[test]
    fn test_float_int_addition() {
        let mut values = Data::default();
        let a = Code::new_with_operands(
            Byte::Push,
            [values.add_constant(Value::FLOAT(0.8), Type::float()), 0, 0],
        );
        let b = Code::new_with_operands(
            Byte::Push,
            [
                values.add_constant(Value::INTEGER(1), Type::integer()),
                0,
                0,
            ],
        );

        let mut program = Program::new(vec![a, b, Code::new(Byte::Add)]);

        Machine::default().run(&program, &values);
    }
}
