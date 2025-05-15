use common::{
    Value,
    memory::object::Objects,
    program::data::Data,
    types::{Kind, Type},
};
use machine::{NativeAction, NativeLibrary};

#[derive(Default)]
pub struct Coroutine {}

impl Coroutine {
    fn resume(name: &str, coroutine: &Value, arg: Option<Value>, data: &Data) -> NativeAction {
        if let Value::OBJECT(Objects::Coroutine(coro)) = coroutine {
            let (ip, stack) = coro.as_ref().resume();

            NativeAction::Resume(ip, stack.clone(), arg.unwrap_or_default())
        } else {
            NativeAction::Fail(format!(
                "Expected argument #1 of function '{name}' to be a coroutine, but got '{}' instead",
                Type::new(coroutine.into()).output(data),
            ))
        }
    }

    fn value(name: &str, coroutine: &Value, data: &Data) -> NativeAction {
        if let Value::OBJECT(Objects::Coroutine(coro)) = coroutine {
            NativeAction::Push(coro.as_ref().get())
        } else {
            NativeAction::Fail(format!(
                "Expected argument #1 of function '{name}' to be a coroutine, but got '{}' instead",
                Type::new(coroutine.into()).output(data),
            ))
        }
    }

    fn valid(coroutine: &Value) -> NativeAction {
        NativeAction::Push(Value::from(matches!(
            coroutine,
            Value::OBJECT(Objects::Coroutine(_))
        )))
    }
}

impl NativeLibrary for Coroutine {
    fn get_functions(&self, data: &mut Data) -> Vec<(&str, Type)> {
        let t_type = data.add_symbol("T".to_string(), None);
        let any = data.add_type(Type::any());
        let t_arg = data.add_type(Type::new(Kind::Generic(t_type, any)));
        let coroutine = Type::new(Kind::Coroutine(t_arg));

        let mut resume_type = Type::function();
        resume_type.add(data.add_type(Type::integer()));
        resume_type.add(data.add_type(coroutine));
        resume_type.set_return(data.add_type(coroutine));

        let mut value_type = Type::function();
        value_type.add(data.add_type(coroutine));
        value_type.set_return(t_arg);

        let mut complete_type = Type::function();
        complete_type.set_return(data.add_type(Type::bool()));
        complete_type.add(data.add_type(Type::any()));

        vec![
            ("std::coroutine::resume", resume_type),
            ("std::coroutine::value", value_type),
            ("std::coroutine::is_coroutine", complete_type),
            // ("std::coroutine::is_complete", complete_type),
            // ...
        ]
    }

    fn call(&self, name: &str, data: &Data, args: &[Value]) -> NativeAction {
        let coroutine = args[0];

        match name {
            "std::coroutine::resume" => Self::resume(name, &coroutine, args.get(1).copied(), data),
            "std::coroutine::value" => Self::value(name, &coroutine, data),
            "std::coroutine::is_coroutine" => Self::valid(&coroutine),
            _ => NativeAction::Fail(format!(
                "Unable to invoke function '{name}' as it is not defined, only declared",
            )),
        }
    }
}
