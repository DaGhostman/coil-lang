//! Sink for emitting `Byte`s into either a `Vec` or [`CodeBuf`].

use common::Byte;

use super::CodeBuf;

/// Minimal push API shared by local `Vec<Byte>` fragments and [`CodeBuf`].
pub trait EmitBuf {
    fn push_byte(&mut self, b: Byte);
    fn push(&mut self, b: Byte) {
        self.push_byte(b);
    }
    fn extend_from_slice_bytes(&mut self, bytes: &[Byte]) {
        for &b in bytes {
            self.push_byte(b);
        }
    }
}

impl EmitBuf for Vec<Byte> {
    fn push_byte(&mut self, b: Byte) {
        self.push(b);
    }
}

impl EmitBuf for CodeBuf {
    fn push_byte(&mut self, b: Byte) {
        self.push(b);
    }
}
