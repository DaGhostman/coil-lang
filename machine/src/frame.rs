use common::vec_array::VecArray;

#[derive(Clone, Debug)]
#[derive(Default)]
pub struct Frame {
    ip: usize,
    stack: usize,
    variables: VecArray<usize, 8>,
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

    pub fn clear(&mut self) {
        self.ip = 0;
        self.stack = 0;
        self.variables = VecArray::default();
    }

    pub fn overwrite(&mut self, symbol: usize, position: usize) {
        self.variables.insert(symbol, position);
    }

    // pub fn lookup(&self, index: usize) -> usize {
    //     *self.variables.get(index)
    // }
}
