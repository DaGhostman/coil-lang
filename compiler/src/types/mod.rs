pub mod ty;
pub mod substitution;
pub mod constraint;
pub mod unify;
pub mod env;

pub use ty::{Type, TypeVar, StructDef};
pub use substitution::Substitution;
pub use constraint::ConstraintSet;
pub use env::TypeEnv;