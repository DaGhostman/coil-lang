use common::{error::Error, program::data::Data, types::Type};

use crate::CompilationPass;

#[derive(Debug, Default)]
pub struct TypeChecker {
    _types: Vec<Type>,
}

impl CompilationPass for TypeChecker {
    fn compile(
        &mut self,
        code: &[common::opcodes::Code],
        _data: &mut Data,
    ) -> Result<Vec<common::opcodes::Code>, Error> {
        Ok(code.to_owned())
        // let mut variables: HashMap<usize, Type> = HashMap::new();
        //
        // for op in program.code() {
        //     match op.code() {
        //         Operation::Const => {
        //             match program
        //                 .constant(op.get(0).copied().unwrap_or_default())
        //                 .map(|v| v.kind())
        //             {
        //                 Some(ValueKind::BOOLEAN(_)) => self.types.push(Type::Bool),
        //                 Some(ValueKind::INTEGER(_)) => self.types.push(Type::Integer),
        //                 Some(ValueKind::FLOAT(_)) => self.types.push(Type::Float),
        //                 Some(ValueKind::STRING(_)) => self.types.push(Type::String),
        //                 Some(ValueKind::NONE) => self.types.push(Type::None),
        //                 Some(ValueKind::FUNCTION(_, _)) => self.types.push(Type::Function),
        //                 a => {
        //                     return Err(Error::new(
        //                         common::error::ErrorOrigin::RUNTIME,
        //                         "Unknown type".to_string(),
        //                     ));
        //                 }
        //             }
        //         }
        //
        //         Operation::Add | Operation::Subtract | Operation::Multiply | Operation::Divide => {
        //             match (self.types.pop(), self.types.pop()) {
        //                 (Some(Type::Integer), Some(Type::Integer)) => {
        //                     self.types.push(Type::Integer);
        //                 }
        //                 (Some(Type::Float), Some(Type::Float)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::Integer), Some(Type::Float)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::Float), Some(Type::Integer)) => {
        //                     self.types.push(Type::Float);
        //                 }
        //                 (Some(Type::String), Some(Type::String)) => {
        //                     self.types.push(Type::String);
        //                 }
        //                 _ => todo!("Not implemented operation check"),
        //             }
        //         }
        //         Operation::Modulo
        //         | Operation::BitAnd
        //         | Operation::BitOr
        //         | Operation::BitXor
        //         | Operation::LeftShift
        //         | Operation::RightShift => {
        //             if let (Some(Type::Integer), Some(Type::Integer)) =
        //                 (self.types.pop(), self.types.pop())
        //             {
        //                 self.types.push(Type::Integer);
        //             } else {
        //                 todo!("Operation not supported for types other than integers")
        //             }
        //         }
        //         Operation::Equal
        //         | Operation::Less
        //         | Operation::LessEqual
        //         | Operation::Greater
        //         | Operation::GreaterEqual => {
        //             if let (Some(rhs), Some(lhs)) = (self.types.pop(), self.types.pop()) {
        //                 if lhs != rhs {
        //                     todo!("Unable to handle comparison between incompatible types")
        //                 } else {
        //                     self.types.push(Type::Bool)
        //                 }
        //             }
        //         }
        //         Operation::Argument => {
        //             // name, type, offset
        //             variables.insert(
        //                 op.get(0).cloned().unwrap_or(0),
        //                 (op.get(1).cloned().unwrap_or(0)).into(),
        //             );
        //         }
        //         Operation::Load => {
        //             self.types.push(
        //                 variables
        //                     .get(&op.operands()[0])
        //                     .cloned()
        //                     .unwrap_or_default(),
        //             );
        //         }
        //         _ => (),
        //     }
        // }
    }
}
