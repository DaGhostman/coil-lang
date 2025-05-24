use common::{
    Value,
    memory::object::Objects,
    native::{Action, Library, Native, NativeFunction},
    program::data::Data,
    types::{Kind, Type},
};

#[derive(Default)]
pub struct Coroutine {}

impl Coroutine {
    fn resume(args: &[Value], data: &Data) -> Action {
        if let &[coroutine] = args {
            if let Value::OBJECT(Objects::Coroutine(coro)) = coroutine {
                let (ip, stack) = coro.as_ref().resume();

                Action::Resume(ip, stack.clone(), args.get(1).copied().unwrap_or_default())
            } else {
                Action::Fail(format!(
                    "Expected argument #1 of function 'std::coroutine::resume' to be a coroutine, but got '{}' instead",
                    Type::new(coroutine.into()).output(data),
                ))
            }
        } else {
            Action::Fail(
                "Argument to function 'std::coroutine::resume' needs to be a coroutine".to_string(),
            )
        }
    }

    fn value<'func>(args: &[Value], data: &Data) -> Action {
        if let &[coroutine] = args {
            if let Value::OBJECT(Objects::Coroutine(coro)) = coroutine {
                Action::Push(coro.as_ref().get())
            } else {
                Action::Fail(format!(
                    "Expected argument #1 of function 'std::coroutine::value' to be a coroutine, but got '{}' instead",
                    Type::new(coroutine.into()).output(data),
                ))
            }
        } else {
            Action::Fail(format!(
                "Not enough arguments to function 'std::coroutine::value'"
            ))
        }
    }

    fn valid(args: &[Value], _: &Data) -> Action {
        if let &[coroutine] = args {
            Action::Push(Value::from(matches!(
                coroutine,
                Value::OBJECT(Objects::Coroutine(_))
            )))
        } else {
            Action::Fail(format!(
                "Not enough arguments to functin 'std::coroutine::valid'"
            ))
        }
    }
}

impl Library for Coroutine {
    fn get_functions(&self, data: &mut Data) -> Vec<Native> {
        let t_type = data.add_symbol("T".to_string(), None);
        let any = data.add_type(Type::any());
        let t_arg = data.add_type(Type::new(Kind::Generic(t_type, any)));
        let coroutine = Type::new(Kind::Coroutine(t_arg));

        let mut resume = Native::new(
            data.add_symbol("std::coroutine::resume".to_string(), None),
            Self::resume,
        );
        resume
            .get_type_mut()
            .add(data.add_type(Type::integer()))
            .add(data.add_type(coroutine))
            .set_return(data.add_type(coroutine));

        let mut value = Native::new(
            data.add_symbol("std::coroutine::value".to_string(), None),
            Self::value,
        );
        value
            .get_type_mut()
            .add(data.add_type(coroutine))
            .set_return(t_arg);

        let mut complete = Native::new(
            data.add_symbol("std::coroutine::is_coroutine".to_string(), None),
            Self::valid,
        );
        complete
            .get_type_mut()
            .add(data.add_type(Type::bool()))
            .set_return(any);

        vec![
            resume, value, complete, // ...
        ]
    }
}
