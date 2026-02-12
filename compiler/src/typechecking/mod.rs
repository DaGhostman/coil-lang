use ahash::HashMap;
use parser::{SimpleSpan, ast::Output};

#[derive(Default)]
pub enum Type {
    #[default]
    Unknown,
    // --
    Integer,
    Float,
    String,
    Boolean,
    Func {
        arg: Box<Type>,
        returns: Box<Type>,
    }, // Functions should be represented as chain
}

#[derive(Default)]
pub struct Env {
    variables: HashMap<String, (Type, SimpleSpan)>,
    functions: HashMap<String, (Type, SimpleSpan)>,
    stack: Vec<Type>,
}

pub struct Checker {}

impl Checker {
    pub fn infer(&self, _expr: Output) -> Result<(), ()> {
        Err(())
    }
}
