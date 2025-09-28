#[repr(u8)]
#[derive(Default, Copy, Clone)]
pub enum FrameState {
    #[default]
    PENDING,
    SUSPENDED,
    STARTED,
    COMPLETE,
    TERMINATED,
}

impl From<(usize, usize)> for Frame {
    fn from(value: (usize, usize)) -> Self {
        Self { ip: value.0, sp: value.1 }
    }
}

impl Into<(usize, usize)> for Frame {
    fn into(self) -> (usize, usize) {
        (self.ip, self.sp)
    }
}

pub struct Frame {
    ip: usize,
    sp: usize,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            ip: 0,
            sp: 0,
        }
    }
}

impl Frame {
    #[inline]
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
    pub fn get(&self) -> usize {
        self.sp
    }

    #[inline]
    pub fn enter(&mut self) {
        self.seek(0);
    }
}
