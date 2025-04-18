pub mod ffi;
pub mod options;
mod utils;

pub mod stack;

pub use options::MachineOptions;
pub use stack::Machine;
//
// use std::{
//     ffi::{CStr, c_void},
//     string::FromUtf8Error,
// };
//
// pub struct FFIResult {
//     value: *const c_void,
// }
//
// impl FFIResult {
//     pub fn new(value: *const c_void) -> Self {
//         Self { value }
//     }
// }
//
// impl From<FFIResult> for usize {
//     fn from(val: FFIResult) -> Self {
//         val.value as *const _ as usize
//     }
// }
//
// impl From<FFIResult> for isize {
//     fn from(val: FFIResult) -> Self {
//         val.value as *const _ as isize
//     }
// }
//
// impl TryInto<String> for FFIResult {
//     type Error = FromUtf8Error;
//
//     fn try_into(self) -> Result<String, Self::Error> {
//         String::from_utf8(unsafe { CStr::from_ptr(self.value as *const _).to_bytes().to_vec() })
//     }
// }
//
// use libloading::{Error, Library, Symbol};
// pub struct DynamicLibrary {
//     lib: Library,
// }
//
// impl DynamicLibrary {
//     pub fn load(path: &str) -> Result<Self, Error> {
//         let lib = unsafe { Library::new(path)? };
//
//         Ok(DynamicLibrary { lib })
//     }
//
//     pub fn do_call(&self, name: &str) -> Result<*const c_void, Error> {
//         let func: Symbol<unsafe extern "C" fn() -> *const c_void> =
//             unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func()) }
//     }
//
//     pub fn do_call1(&self, name: &str, args: &[*const c_void]) -> Result<*const c_void, Error> {
//         let func: Symbol<unsafe extern "C" fn(*const c_void) -> *const c_void> =
//             unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func(args[0])) }
//     }
//
//     pub fn do_call2(&self, name: &str, args: &[*const c_void]) -> Result<*const c_void, Error> {
//         let func: Symbol<unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void> =
//             unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func(args[0], args[1])) }
//     }
//
//     pub fn do_call3(&self, name: &str, args: &[*const c_void]) -> Result<*const c_void, Error> {
//         let func: Symbol<
//             unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> *const c_void,
//         > = unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func(args[0], args[1], args[2])) }
//     }
//     pub fn do_call4(&self, name: &str, args: &[*const c_void]) -> Result<*const c_void, Error> {
//         let func: Symbol<
//             unsafe extern "C" fn(
//                 *const c_void,
//                 *const c_void,
//                 *const c_void,
//                 *const c_void,
//             ) -> *const c_void,
//         > = unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func(args[0], args[1], args[2], args[3])) }
//     }
//
//     pub fn do_call5(&self, name: &str, args: &[*const c_void]) -> Result<*const c_void, Error> {
//         let func: Symbol<
//             unsafe extern "C" fn(
//                 *const c_void,
//                 *const c_void,
//                 *const c_void,
//                 *const c_void,
//                 *const c_void,
//             ) -> *const c_void,
//         > = unsafe { self.lib.get(name.as_bytes())? };
//
//         unsafe { Ok(func(args[0], args[1], args[2], args[3], args[4])) }
//     }
//
//     pub fn call(&self, name: &str, args: &[*const c_void]) -> Result<FFIResult, Error> {
//         Ok(FFIResult::new(match args.len() {
//             0 => self.do_call(name)?,
//             1 => self.do_call1(name, args)?,
//             2 => self.do_call2(name, args)?,
//             3 => self.do_call3(name, args)?,
//             4 => self.do_call4(name, args)?,
//             5 => self.do_call5(name, args)?,
//             _ => panic!("Unable to use FFI with more than 5 arguments at a time"),
//         }))
//     }
// }
