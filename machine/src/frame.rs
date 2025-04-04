use common::vec_array::VecArray;

#[derive(Clone)]
pub struct Frame {
    ip: usize,
    stack: usize,
    variables: VecArray<usize, 32>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            ip: 0,
            stack: 0,
            variables: VecArray::default(),
        }
    }
}

impl Frame {
    pub fn tell(&self) -> usize {
        self.ip
    }

    pub fn stack(&self) -> usize {
        self.stack
    }

    pub fn replace(&mut self, ip: usize, stack: usize) {
        self.ip = ip;
        self.stack = stack;
    }

    pub fn overwrite(&mut self, symbol: usize, position: usize) {
        *self.variables.get_mut(symbol) = position;
    }

    pub fn lookup(&self, index: usize) -> usize {
        *self.variables.get(index)
    }
}
