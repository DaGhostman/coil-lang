pub struct Stack<T, const N: usize>
where
    T: Copy,
{
    stack: [T; N],
    sp: usize,
}

impl<T, const N: usize> Default for Stack<T, N>
where
    T: Copy + Default,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Stack<T, N>
where
    T: Copy + Default,
{
    #[must_use]
    pub fn new() -> Self {
        Self {
            sp: 0,
            stack: [Default::default(); N],
        }
    }

    pub fn pop(&mut self) -> &T {
        assert!(self.sp > 0);

        self.sp -= 1;
        &self.stack[self.sp]
    }

    pub fn npop(&mut self, n: usize) -> &[T] {
        assert!(self.sp > n);
        let boundary = self.sp;
        assert!(boundary > n);
        self.sp -= n;

        &self.stack[self.sp..boundary]
    }

    pub fn push(&mut self, value: T) {
        assert!(self.sp < N);

        self.stack[self.sp] = value;
        self.sp += 1;
    }

    pub fn insert(&mut self, idx: usize, value: T) {
        self.sp = self.sp.max(idx + 1);

        self.stack[idx] = value;
    }

    pub fn tell(&self, offset: usize) -> usize {
        self.sp - offset
    }

    pub fn restore(&mut self, size: usize) {
        assert!(size < self.sp);
        assert!(self.sp > 0);

        self.stack[size] = self.stack[self.sp - 1];
        self.sp = size + 1;
    }

    pub fn peek(&self, offset: usize) -> &T {
        assert!(self.sp - offset > 0);

        &self.stack[self.sp - 1 - offset]
    }

    pub fn get(&self, position: usize) -> &T {
        assert!(position < N);

        &self.stack[position]
    }

    pub fn set(&mut self, position: usize, value: T) {
        assert!(position < N);

        self.stack[position] = value;
    }

    pub fn len(&self) -> usize {
        self.sp
    }

    pub fn iter(&self) -> std::slice::Iter<T> {
        assert!(self.sp < N);

        self.stack[..self.sp].iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<T> {
        assert!(self.sp < N);

        self.stack[..self.sp].iter_mut()
    }
}
