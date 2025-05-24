use crate::guarantee;

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
        debug_assert!(self.sp < N);
        debug_assert!(self.sp > 0);

        self.sp -= 1;
        &self.stack[self.sp]
    }

    pub fn rewind_by(&mut self, n: usize) {
        debug_assert!(self.sp > 0);
        self.sp -= n;
    }

    pub fn npop(&mut self, n: usize) -> &[T] {
        debug_assert!(self.sp > 0);
        let boundary = self.sp;
        self.sp -= n;

        &self.stack[self.sp..boundary]
    }

    pub fn push(&mut self, value: T) {
        debug_assert!(self.sp < N);

        self.stack[self.sp] = value;
        self.sp += 1;
    }

    pub fn insert(&mut self, idx: usize, value: T) {
        debug_assert!(self.sp < N);
        debug_assert!(idx < N);

        let items = self.stack[idx..self.sp].to_vec();
        self.sp -= idx;

        self.stack[idx] = value;
        self.sp += 1;

        for val in items {
            self.stack[self.sp] = val;
            self.sp += 1;
        }
    }

    pub fn tell(&self, offset: usize) -> usize {
        debug_assert!(self.sp >= offset);
        self.sp - offset
    }

    pub fn restore(&mut self, size: usize) {
        debug_assert!(self.sp > 0);
        debug_assert!(size <= self.sp);
        debug_assert!(self.sp + 1 < N);

        self.stack[size] = self.stack[self.sp - 1];
        self.sp = size + 1;
    }

    pub fn peek(&self, offset: usize) -> &T {
        debug_assert!(self.sp >= 1);

        &self.stack[self.sp - 1 - offset]
    }

    pub fn get(&self, position: usize) -> &T {
        debug_assert!(position < N);

        &self.stack[position]
    }

    pub fn set(&mut self, position: usize, value: T) {
        debug_assert!(position < N);

        self.stack[position] = value;
        self.sp = self.sp.max(position + 1);
    }

    pub fn len(&self) -> usize {
        self.sp
    }

    pub fn iter(&self) -> std::slice::Iter<T> {
        guarantee!(self.sp < N);

        self.stack[..self.sp].iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<T> {
        guarantee!(self.sp < N);
        unsafe {
            std::hint::assert_unchecked(self.sp < N);
        }

        self.stack[..self.sp].iter_mut()
    }
}
