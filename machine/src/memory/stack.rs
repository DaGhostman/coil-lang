//! Fixed-size operand stack with an explicit cursor (`tell` / `seek`).

use std::ops::{Index, IndexMut, Range};

use common::promise;

pub struct Stack<T: Default, const N: usize> {
    stack: [T; N],
    cursor: usize,
}

impl<T: Default + Copy, const N: usize> Default for Stack<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default + Copy, const N: usize> Stack<T, N> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: std::array::from_fn(|_| T::default()),
            cursor: 0,
        }
    }

    #[inline]
    pub fn pop(&mut self) -> T {
        promise!(self.cursor > 0);
        self.cursor -= 1;
        // SAFETY: cursor was in `1..=N` before decrement.
        unsafe { *self.stack.get_unchecked(self.cursor) }
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        promise!(self.cursor < N);
        // SAFETY: promise! guarantees cursor < N.
        unsafe {
            *self.stack.get_unchecked_mut(self.cursor) = value;
        }
        self.cursor += 1;
    }

    #[inline]
    pub fn peek(&self) -> &T {
        promise!(self.cursor > 0);
        // SAFETY: cursor > 0 and ≤ N.
        unsafe { self.stack.get_unchecked(self.cursor - 1) }
    }

    #[inline]
    pub fn seek(&mut self, idx: usize) {
        promise!(idx <= N);
        self.cursor = idx;
    }

    #[inline]
    pub fn top(&mut self) -> &mut T {
        promise!(self.cursor > 0);
        // SAFETY: cursor > 0 and ≤ N.
        unsafe { self.stack.get_unchecked_mut(self.cursor - 1) }
    }

    /// Duplicate TOS without going through `peek` + `push`.
    #[inline]
    pub fn duplicate(&mut self) {
        promise!(self.cursor > 0);
        promise!(self.cursor < N);
        // SAFETY: both indices are in-bounds by the promises above.
        unsafe {
            let v = *self.stack.get_unchecked(self.cursor - 1);
            *self.stack.get_unchecked_mut(self.cursor) = v;
        }
        self.cursor += 1;
    }

    /// Copy `len` values from `src` to `dst` (forward; caller ensures non-overlap or `dst <= src`).
    #[inline]
    pub fn copy_slots(&mut self, dst: usize, src: usize, len: usize) {
        promise!(dst + len <= N);
        promise!(src + len <= N);
        if dst == src || len == 0 {
            return;
        }
        // SAFETY: ranges fit in the fixed buffer.
        unsafe {
            let ptr = self.stack.as_mut_ptr();
            std::ptr::copy(ptr.add(src), ptr.add(dst), len);
        }
    }

    #[inline]
    pub fn tell(&self) -> usize {
        self.cursor
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.stack[..self.cursor]
    }

    /// Full backing storage (including slots below the cursor).
    /// Used by the GC to root live locals that share the operand stack.
    #[inline]
    pub fn buffer(&self) -> &[T] {
        &self.stack
    }
}

impl<T: Default, const N: usize> Index<Range<usize>> for Stack<T, N> {
    type Output = [T];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        promise!(index.end <= N);
        promise!(index.start <= index.end);
        // SAFETY: range is within the fixed buffer.
        unsafe { self.stack.get_unchecked(index) }
    }
}

impl<T: Default + Copy, const N: usize> IntoIterator for Stack<T, N> {
    type Item = T;
    type IntoIter = std::array::IntoIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {
        self.stack.into_iter()
    }
}

impl<T: Default + Copy, const N: usize> Index<usize> for Stack<T, N> {
    type Output = T;
    fn index(&self, index: usize) -> &Self::Output {
        promise!(index < N);
        // SAFETY: promise! guarantees index < N.
        unsafe { self.stack.get_unchecked(index) }
    }
}

impl<T: Default + Copy, const N: usize> IndexMut<usize> for Stack<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        promise!(index < N);
        // SAFETY: promise! guarantees index < N.
        unsafe { self.stack.get_unchecked_mut(index) }
    }
}

#[cfg(debug_assertions)]
impl<T: Default + std::fmt::Debug, const N: usize> std::fmt::Debug for Stack<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", &self.stack[..self.cursor])
    }
}
