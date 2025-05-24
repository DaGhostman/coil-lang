use crate::{Value, memory::Heap, program::data::Data, types::Type};

type AllocateFn = fn(&mut Heap, &Data, Vec<Value>) -> Value;

#[derive(Default)]
pub enum Action {
    #[default]
    None,
    Push(Value),
    Resume(usize, Vec<Value>, Value),
    Allocate(AllocateFn, Option<Vec<Value>>),
    Fail(String),
}

pub type NativeFunction = fn(&[Value], &Data) -> Action;

#[macro_export]
macro_rules! native {
    ($data: expr, $name: expr, $handler: expr, $type_handler:expr) => {{
        let symbol = $data.add_symbol($name.to_string(), None);
        let mut callback = Native::new(symbol, $handler);
        $type_handler(&mut callback.get_type_mut());

        callback
    }};
}

#[derive(Copy, Clone, Debug)]
pub struct Native {
    name: usize,
    func: NativeFunction,
    r#type: Type,
}

impl Native {
    pub fn new(name: usize, func: NativeFunction) -> Self {
        Self {
            func,
            r#type: Type::function(),
            name,
        }
    }

    pub fn get_name(&self) -> usize {
        self.name
    }

    pub fn name(&self, data: &Data) -> String {
        data.symbol_name(self.name).to_owned()
    }

    pub fn get_type_mut(&mut self) -> &mut Type {
        &mut self.r#type
    }

    pub fn get_type(&self) -> Type {
        self.r#type
    }

    pub fn r#type(&self, data: &Data) -> usize {
        data.find_type(self.r#type)
    }

    pub fn get_arity(&self) -> usize {
        self.r#type.len()
    }

    pub fn call(&self, args: &[Value], data: &Data) -> Action {
        (self.func)(args, data)
    }
}

pub trait Library {
    fn get_functions(&self, data: &mut Data) -> Vec<Native>;
}
