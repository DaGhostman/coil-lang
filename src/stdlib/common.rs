use common::native::{Library, Native, NativeFunction};

#[derive(Default)]
pub struct Common {}

impl Library for Common {
    fn get_functions(&self, _data: &mut common::program::data::Data) -> Vec<Native> {
        vec![]
    }
}
