use crate::{Frame, garbage::GcSized};

pub struct Coroutine<T: Default>(Frame<T>);

impl<T: Default> Coroutine<T> {
    pub fn new(frame: Frame<T>) -> Self {
        Self(frame)
    }

    pub fn frame(&self) -> &Frame<T> {
        &self.0
    }

    pub fn frame_mut(&mut self) -> &mut Frame<T> {
        &mut self.0
    }
}

impl<T: Default> GcSized for Coroutine<T> {
    fn size(&self) -> usize {
        use std::mem::size_of_val;

        size_of_val(&self.0)
    }
}

#[cfg(debug_assertions)]
impl<T: Default + std::fmt::Debug> std::fmt::Debug for Coroutine<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#[{:?}]", self.0)
    }
}
