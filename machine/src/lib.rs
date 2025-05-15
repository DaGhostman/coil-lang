pub mod ffi;
pub mod options;
mod utils;

pub mod stack;

use common::{Value, program::data::Data, types::Type};
pub use options::MachineOptions;
pub use stack::Machine;

#[derive(Default)]
pub enum NativeAction {
    #[default]
    None,
    Push(Value),
    Resume(usize, Vec<Value>, Value),
    Fail(String),
}

pub trait NativeLibrary {
    fn get_functions(&self, data: &mut Data) -> Vec<(&str, Type)>;
    fn call(&self, name: &str, data: &Data, args: &[Value]) -> NativeAction;
}
