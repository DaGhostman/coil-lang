pub mod ffi;
pub mod options;
mod utils;

pub mod stack;

// use common::{
//     // Value,
//     // memory::{Heap, object::ObjString},
//     program::data::Data,
//     types::Type,
// };
pub use options::MachineOptions;
pub use stack::Machine;

// type AllocateFn<'allocate> = &'allocate dyn Fn(&mut Heap, Vec<Value>) -> Value;
//
// #[derive(Default)]
// pub enum NativeAction<'allocation> {
//     #[default]
//     None,
//     Push(Value),
//     Resume(usize, Vec<Value>, Value),
//     AllocateString(ObjString),
//     Allocate(AllocateFn<'allocation>, Option<Vec<Value>>),
//     Fail(String),
// }
//
// pub trait NativeLibrary {
//     fn get_functions(&self, data: &mut Data) -> Vec<(&str, Type)>;
//     fn call(&self, name: &str, data: &Data, args: &[Value]) -> NativeAction;
// }
