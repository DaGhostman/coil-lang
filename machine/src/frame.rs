const MAX_VARIABLES: usize = 32;

#[derive(Clone, Debug, Default)]
struct Variables {
    storage: [usize; MAX_VARIABLES],
    expansion: Vec<usize>,
}

impl Variables {
    pub fn get(&self, symbol: usize) -> usize {
        if symbol > MAX_VARIABLES {
            if self.expansion.len() <= symbol {
                unreachable!("Attampting to get unknown variable")
            }

            self.expansion[symbol]
        } else {
            self.storage[symbol]
        }
    }

    pub fn set(&mut self, symbol: usize, position: usize) {
        if symbol < MAX_VARIABLES {
            self.storage[symbol] = position;
        } else {
            let length = self.expansion.len();
            if self.expansion.len() <= symbol {
                self.expansion.resize(length + 16, Default::default());
            }

            self.expansion[symbol] = position;
        }
    }
}

#[derive(Clone, Debug)]
#[derive(Default)]
pub struct Frame {
    ip: usize,
    stack: usize,
    variables: Variables,
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
        // self.variables.clear();
    }

    pub fn overwrite(&mut self, symbol: usize, position: usize) {
        self.variables.set(symbol, position);
    }

    pub fn lookup(&self, index: usize) -> usize {
        self.variables.get(index)
    }
}
