pub mod type;
pub mod substitution;
pub mod constraint;
pub mod unify;
pub mod env;

pub use type::{Type, TypeVar, StructDef, InterfaceDef, Field, Method, GenericType, TypeAlias, Variant};
pub use substitution::Substitution;
pub use constraint::{Constraint, ConstraintSet};
pub use env::TypeEnv;