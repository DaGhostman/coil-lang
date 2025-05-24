use common::{
    Value, ValuePtr,
    error::{Message, MessageOrigin},
    program::data::Data,
    types::Kind,
};
use rustc_hash::FxHashMap as HashMap;

use libffi::{
    low::{CodePtr, call, prep_cif},
    raw::{ffi_abi_FFI_DEFAULT_ABI, ffi_cif, ffi_type},
};
use libloading::{Library, Symbol};
use std::ffi::c_void;

#[derive(Debug)]
pub struct DynamicLibrary {
    lib: Library,
    functions: HashMap<usize, FFIFunction>,
}

#[derive(Default, Debug)]
pub struct FFIFunction {
    name: String,
    arguments: Vec<Kind>,
    return_type: Kind,
}

impl FFIFunction {
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            arguments: Vec::with_capacity(4),
            return_type: Kind::default(),
        }
    }

    pub fn add_argument(&mut self, type_: Kind) -> &mut Self {
        self.arguments.push(type_);

        self
    }

    pub fn returns(&mut self, type_: Kind) -> &mut Self {
        self.return_type = type_;

        self
    }

    pub fn call(
        &self,
        lib: &Library,
        arguments: &[Value],
        data: &mut Data,
    ) -> Result<Value, Message> {
        if arguments.len() != self.arguments.len() {
            return Err(Message::error(
                MessageOrigin::FFI,
                format!(
                    "Function expected {} arguments, but called with {}",
                    self.arguments.len(),
                    arguments.len()
                ),
            ));
        }

        for (idx, argument) in arguments.iter().enumerate() {
            if Some(&(argument.to_owned()).into()) != self.arguments.get(idx)
                && self.arguments.get(idx) != Some(&Kind::Pointer)
            {
                return Err(Message::error(
                    MessageOrigin::FFI,
                    format!(
                        "Expected argument #{} to be of type '{}', but '{}' was received",
                        idx + 1,
                        self.arguments.get(idx).expect("Unable to determine type"),
                        argument
                    ),
                ));
            }
        }

        let mut arguments: Vec<ValuePtr> = arguments[0..self.arguments.len()]
            .iter()
            .map(|value| value.ptr(data).expect("Unable to get value pointer."))
            .collect();

        let mut argument_bag: Vec<*mut c_void> = vec![];
        let mut types: Vec<*mut ffi_type> = vec![];

        for (idx, arg) in arguments.iter_mut().enumerate() {
            types.insert(
                idx,
                Box::into_raw(Box::from(<Kind as Into<ffi_type>>::into(arg.kind()))),
            );

            argument_bag.insert(idx, arg.ptr_mut::<c_void>());
        }

        let function = format!("{}\0", self.name);
        let func: Symbol<*mut c_void> = unsafe {
            lib.get(function.as_bytes())
                .map_err(|e| Message::error(MessageOrigin::FFI, format!("{e}")))?
        };

        unsafe {
            let ptr = func.into_raw().into_raw();
            let mut cif: ffi_cif = Default::default();

            prep_cif(
                &mut cif,
                ffi_abi_FFI_DEFAULT_ABI,
                self.arguments.len(),
                &mut self.return_type.into(),
                types.as_mut_ptr(),
            )
            .expect("Unable to prepare library");
            let result =
                call::<*mut c_void>(&mut cif, CodePtr::from_ptr(ptr), argument_bag.as_mut_ptr());

            for t in types {
                drop(Box::from_raw(t));
            }

            Ok(match self.return_type {
                Kind::Resource | Kind::Pointer => Value::pointer(result),
                _ => Value::from_ptr_and_type(result, self.return_type, data),
            })
        }
    }

    #[must_use]
    pub fn arity(&self) -> usize {
        self.arguments.len()
    }
}

impl DynamicLibrary {
    pub fn load(name: &str) -> Result<Self, Message> {
        let lib = unsafe {
            Library::new(name).map_err(|e| {
                Message::error(
                    MessageOrigin::FFI,
                    format!("Unable to load dynamic library '{name}': {e}"),
                )
            })?
        };

        Ok(Self {
            lib,
            functions: HashMap::default(),
        })
    }

    pub fn add_function(&mut self, symbol: usize, name: String) -> &mut FFIFunction {
        self.functions
            .entry(symbol)
            .or_insert_with(|| FFIFunction::new(name))
    }

    #[must_use]
    pub fn function(&self, symbol: usize) -> Option<&FFIFunction> {
        self.functions.get(&symbol)
    }

    pub fn function_mut(&mut self, symbol: usize) -> Option<&mut FFIFunction> {
        self.functions.get_mut(&symbol)
    }

    pub fn call(&self, name: usize, args: &[Value], data: &mut Data) -> Result<Value, Message> {
        if let Some(function) = self.functions.get(&name) {
            Ok(function.call(&self.lib, args, data)?)
        } else {
            Err(Message::error(
                MessageOrigin::RUNTIME,
                "Requested function does not exist in the provided FFI module".to_owned(),
            ))
        }
    }
}
