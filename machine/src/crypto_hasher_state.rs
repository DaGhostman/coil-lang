//! Incremental digest state for host `CryptoHasher` handles.

use blake3::Hasher as Blake3Hasher;
use sha2::{Digest, Sha256, Sha512};

use crate::memory::GcSized;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HasherAlg {
    Sha256 = 0,
    Sha512 = 1,
    Blake3 = 2,
}

impl HasherAlg {
    pub fn from_tag(tag: i64) -> Option<Self> {
        match tag {
            0 => Some(Self::Sha256),
            1 => Some(Self::Sha512),
            2 => Some(Self::Blake3),
            _ => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "sha256" | "sha-256" => Some(Self::Sha256),
            "sha512" | "sha-512" => Some(Self::Sha512),
            "blake3" => Some(Self::Blake3),
            _ => None,
        }
    }
}

pub enum StreamingHasher {
    Sha256(Sha256),
    Sha512(Sha512),
    Blake3(Blake3Hasher),
}

impl StreamingHasher {
    pub fn new(alg: HasherAlg) -> Self {
        match alg {
            HasherAlg::Sha256 => Self::Sha256(Sha256::new()),
            HasherAlg::Sha512 => Self::Sha512(Sha512::new()),
            HasherAlg::Blake3 => Self::Blake3(Blake3Hasher::new()),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha256(h) => h.update(data),
            Self::Sha512(h) => h.update(data),
            Self::Blake3(h) => {
                h.update(data);
            }
        }
    }

    pub fn finalize(self) -> Vec<u8> {
        match self {
            Self::Sha256(h) => h.finalize().to_vec(),
            Self::Sha512(h) => h.finalize().to_vec(),
            Self::Blake3(h) => h.finalize().as_bytes().to_vec(),
        }
    }
}

/// Opaque incremental digest handle on the VM heap.
pub struct ObjCryptoHasher {
    pub state: Option<StreamingHasher>,
}

impl ObjCryptoHasher {
    pub fn new(alg: HasherAlg) -> Self {
        Self {
            state: Some(StreamingHasher::new(alg)),
        }
    }
}

impl GcSized for ObjCryptoHasher {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
