#[derive(Clone, Debug, Default)]
pub struct Frame {
    ip: usize,
    stack: usize,
    size: usize,
    variables: Vec<usize>,
    used: bool,
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

        if self.used {
            // self.variables.clear();
            self.size = 0;
        }
        self.used = true;
    }

    pub fn overwrite(&mut self, symbol: usize, position: usize) {
        if self.size == symbol {
            self.variables.resize(self.size + 4, usize::MAX);
            self.size += 4;
        }

        self.variables[symbol] = position;
    }

    pub fn lookup(&self, index: usize) -> usize {
        self.variables[index]
    }
}
