#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum Type {
    #[default]
    None,
    Bool,
    Integer,
    Float,
    String,
    Function,
}

impl From<Type> for usize {
    fn from(value: Type) -> Self {
        (value as u8) as usize
    }
}

impl From<usize> for Type {
    fn from(value: usize) -> Self {
        match value {
            0 => Type::None,
            1 => Type::Bool,
            2 => Type::Integer,
            3 => Type::Float,
            4 => Type::String,
            5 => Type::Function,
            _ => Type::None,
        }
    }
}
