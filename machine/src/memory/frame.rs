impl From<(usize, usize)> for Frame {
    fn from(value: (usize, usize)) -> Self {
        Self {
            ip: value.0,
            sp: value.1,
        }
    }
}

impl From<Frame> for (usize, usize) {
    fn from(val: Frame) -> Self {
        (val.ip, val.sp)
    }
}

#[derive(Default)]
pub struct Frame {
    ip: usize,
    sp: usize,
}

impl Frame {
    #[inline]
    #[must_use]
    pub fn tell(&self) -> usize {
        self.ip
    }

    #[inline]
    pub fn seek(&mut self, ip: usize) {
        self.ip = ip;
    }

    #[inline]
    pub fn set(&mut self, sp: usize) {
        self.sp = sp;
    }

    #[inline]
    #[must_use]
    pub fn get(&self) -> usize {
        self.sp
    }

    #[inline]
    pub fn enter(&mut self) {
        self.seek(0);
    }
}
