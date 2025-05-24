use common::{
    Value,
    memory::{
        Heap,
        object::{ObjString, Objects},
    },
    native,
    native::{Action, Library, Native},
    program::data::Data,
    types::Type,
};

#[derive(Default)]
pub struct Basic {}

impl Basic {
    pub fn char(args: &[Value], _: &Data) -> Action {
        if args.len() != 1 {
            return Action::Fail(format!(
                "Function expects 1 argument, but {} received",
                args.len()
            ));
        }
        let num = args[0];

        if !matches!(num, Value::INTEGER(_)) {
            return Action::Fail("Cannot convert non-integer into string".to_string());
        }

        Action::Allocate(
            |vm, _, vals| {
                if let Value::INTEGER(n) = vals[0] {
                    let (o, _) = vm.alloc(
                        ObjString::from((n as u8 as char).to_string()),
                        Objects::String,
                    );

                    Value::OBJECT(o)
                } else {
                    Value::NONE
                }
            },
            Some(vec![num]),
        )
    }

    pub fn ord(args: &[Value], data: &Data) -> Action {
        if let &[char] = args {
            let char_string = match char {
                Value::STR(n) => data.string(n).to_string(),
                Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                value => value.to_string(),
            };

            if char_string.len() > 1 {
                Action::Fail(format!(
                    "Function 'ord' expected a single character, but instead saw {}",
                    char_string.len()
                ))
            } else {
                Action::Push(Value::INTEGER(char_string.as_bytes()[0] as i64))
            }
        } else {
            Action::Fail("Not enough arguments to function 'std::string::ord'".to_string())
        }
    }

    pub fn contains(args: &[Value], data: &Data) -> Action {
        if let &[string, substr] = args {
            let mut search_string = match substr {
                Value::STR(n) => data.string(n).to_string(),
                Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                value => value.to_string(),
            };
            let mut origin_string = match string {
                Value::STR(n) => data.string(n).to_string(),
                Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                value => value.to_string(),
            };

            if matches!(args.get(2), Some(Value::BOOLEAN(true))) {
                search_string = search_string.to_lowercase();
                origin_string = origin_string.to_lowercase();
            }

            Action::Push(Value::BOOLEAN(origin_string.contains(&search_string)))
        } else {
            Action::Fail("Not enough arguments to function 'std::string::contains'".to_string())
        }
    }

    pub fn search(args: &[Value], data: &Data) -> Action {
        if let &[string, substr] = args {
            let mut search_string = match substr {
                Value::STR(n) => data.string(n).to_string(),
                Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                value => value.to_string(),
            };
            let mut origin_string = match string {
                Value::STR(n) => data.string(n).to_string(),
                Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                value => value.to_string(),
            };

            if search_string.len() > origin_string.len() {
                return Action::Push(Value::BOOLEAN(false));
            }

            if matches!(args.get(2), Some(Value::BOOLEAN(true))) {
                search_string = search_string.to_lowercase();
                origin_string = origin_string.to_lowercase();
            }

            Action::Push(Value::INTEGER(
                origin_string
                    .find(&search_string)
                    .map(|v| v as i64)
                    .unwrap_or(-1),
            ))
        } else {
            Action::Fail("Not enough arguments for function 'std::string::search'".to_string())
        }
    }

    pub fn replace(args: &[Value], _: &Data) -> Action {
        Action::Allocate(
            |heap: &mut Heap, data: &Data, vals: Vec<Value>| {
                if let &[string, substr, replacement] = vals.as_slice() {
                    let search_string = match substr {
                        Value::STR(n) => data.string(n).to_string(),
                        Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                        value => value.to_string(),
                    };
                    let origin_string = match string {
                        Value::STR(n) => data.string(n).to_string(),
                        Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                        value => value.to_string(),
                    };
                    let replacement_string = match replacement {
                        Value::STR(n) => data.string(n).to_string(),
                        Value::OBJECT(Objects::String(str)) => str.as_ref().to_string(),
                        value => value.to_string(),
                    };

                    let (val, _) = heap.alloc(
                        ObjString::from(origin_string.replace(&search_string, &replacement_string)),
                        Objects::String,
                    );

                    Value::OBJECT(val)
                } else {
                    Value::NONE
                }

                // if let Value::INTEGER(n) = vals[0] {
                //     let (o, _) = vm.alloc(
                //         ObjString::from((n as u8 as char).to_string()),
                //         Objects::String,
                //     );
                //
                //     Value::OBJECT(o)
                // } else {
                //     Value::NONE
                // }
            },
            Some(args.to_vec()),
        )
        // NativeAction::AllocateString(ObjString::from(
        //     origin_string.replace(&search_string, &replacement_string),
        // ))
    }
}

impl Library for Basic {
    fn get_functions(&self, data: &mut common::program::data::Data) -> Vec<Native> {
        let int = data.add_type(Type::integer());
        let str = data.add_type(Type::string());
        let bool = data.add_type(Type::bool());

        let char_ = native!(data, "std::string::char", Self::char, |ty: &mut Type| {
            ty.add(int).set_return(str);
        });

        let mut ord = Native::new(
            data.add_symbol("std::string::ord".to_string(), None),
            Self::ord,
        );
        ord.get_type_mut().add(str).set_return(int);

        let mut contains = Native::new(
            data.add_symbol("std::string::contains".to_string(), None),
            Self::contains,
        );
        contains.get_type_mut().add(str).add(str).set_return(bool);

        let mut search = Native::new(
            data.add_symbol("std::string::search".to_string(), None),
            Self::search,
        );
        search
            .get_type_mut()
            .add(str)
            .add(str)
            .add(bool)
            .set_return(int);

        let mut replace = Native::new(
            data.add_symbol("std::string::replace".to_string(), None),
            Self::replace,
        );
        replace
            .get_type_mut()
            .add(str)
            .add(str)
            .add(str)
            .set_return(str);

        vec![
            char_, ord, contains, search, replace,
            // ...
        ]
    }
}
