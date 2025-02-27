use ahash::AHashMap as HashMap;

#[derive(Clone, Debug, Default)]
pub struct Frame {
    ip: usize,
    stack: usize,
    // ----
    variables: HashMap<usize, usize>,
}

impl Frame {
    pub fn new(ip: usize, stack: usize) -> Self {
        Frame {
            ip,
            stack,

            variables: HashMap::with_capacity(16),
        }
    }

    pub fn tell(&self) -> usize {
        self.ip
    }

    pub fn stack(&self) -> usize {
        self.stack
    }

    pub fn lookup(&self, key: &usize) -> Option<&usize> {
        self.variables.get(key)
    }

    pub fn store(&mut self, key: &usize, position: usize) {
        self.variables.insert(*key, position);
    }
}
