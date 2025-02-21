#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum Type {
    None,
    Bool,
    Integer,
    Float,
    String,
    Function,
}

impl Into<usize> for Type {
    fn into(self) -> usize {
        (self as u8) as usize
    }
}
