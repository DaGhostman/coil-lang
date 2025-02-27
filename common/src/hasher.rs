use core::hash::Hasher;

#[derive(Default, Copy, Clone)]
pub struct ValueHasher {
    hash: [u8; 8],
}

impl Hasher for ValueHasher {
    fn finish(&self) -> u64 {
        u64::from_ne_bytes(self.hash)
    }

    fn write(&mut self, bytes: &[u8]) {
        for x in 0..8 {
            self.hash[x] = bytes[x];
        }
    }
}
