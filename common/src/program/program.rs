use core::fmt::Debug;

use super::data::Data;

#[derive(Clone, Default, PartialEq)]
pub struct Program<T> {
    length: usize,
    code: Vec<T>,
    data: Data,
}

impl<T> Debug for Program<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.code.iter().for_each(|code| {
            let _ = write!(f, "{code:?} ");
        });

        write!(f, "")
    }
}

impl<T> Program<T>
where
    T: Clone,
{
    #[must_use]
    pub fn new(code: Vec<T>) -> Self {
        Self {
            length: code.len(),
            code,
            data: Data::default(),
        }
    }

    pub fn with_data(&mut self, data: Data) {
        self.data = data;
    }

    pub fn with_code(&mut self, code: Vec<T>) -> bool {
        let len = self.code.len();
        self.code = code;
        self.length = len;

        len == self.code.len()
    }

    pub fn push(&mut self, instruction: T) {
        self.code.push(instruction);
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&T> {
        self.code.get(index)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.length
    }

    #[must_use]
    pub fn code(&self) -> &[T] {
        self.code.as_slice()
    }

    #[must_use]
    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Data {
        &mut self.data
    }
}
