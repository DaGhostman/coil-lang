use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default)]
pub struct Frame {
    ip: usize,
    stack: usize,
    parent: Option<Box<Frame>>,
    scoped: bool,
    // ----
    variables: HashMap<usize, usize>,
    upvalues: HashSet<usize>,
}

impl Frame {
    pub fn new(ip: usize, stack: usize) -> Self {
        Frame {
            ip,
            stack,
            parent: None,
            scoped: false,

            variables: HashMap::default(),
            upvalues: HashSet::default(),
        }
    }

    pub fn with_parent(&mut self, parent: Frame) {
        self.parent = Some(Box::new(parent));
    }

    pub fn tell(&self) -> usize {
        self.ip
    }

    pub fn stack(&self) -> usize {
        self.stack
    }

    pub fn parent(&self) -> Option<&Frame> {
        self.parent.as_deref()
    }

    pub fn scope(&mut self) -> Frame {
        let mut f = self.clone();
        f.stack = 0;
        f.scoped = true;
        f.with_parent(self.clone());

        f
    }

    pub fn is_scoped(&self) -> bool {
        self.scoped
    }

    pub fn lookup(&self, key: usize) -> Option<usize> {
        if let Some(position) = self.variables.get(&key) {
            Some(*position)
        } else if self.upvalues.contains(&key) {
            if let Some(frame) = self.parent.clone() {
                frame.lookup(key)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn store(&mut self, key: usize, position: usize) {
        self.variables.insert(key, position);
    }

    pub fn hoist(&mut self, key: usize) {
        self.upvalues.insert(key);
    }
}
