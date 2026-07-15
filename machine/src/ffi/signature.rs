//! Explicit FFI signatures.

use crate::memory::FfiType;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FfiSignature {
    pub name: String,
    pub args: Vec<FfiType>,
    pub ret: FfiType,
}

impl FfiSignature {
    pub fn arity(&self) -> usize {
        self.args.len()
    }

    pub fn from_parts(
        name: impl Into<String>,
        args: Vec<FfiType>,
        ret: FfiType,
    ) -> Result<Self, FfiError> {
        FfiSignatureBuilder::new(name).args(args).ret(ret).build()
    }
}

#[derive(Debug, Default)]
pub struct FfiSignatureBuilder {
    name: String,
    args: Vec<FfiType>,
    ret: Option<FfiType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfiError {
    MissingName,
    MissingReturnType,
    VoidArgument { index: usize },
    EmptyName,
    Libffi(String),
    ArityMismatch { expected: usize, got: usize },
    SymbolNotFound { name: String },
    Unsupported(String),
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingName => write!(f, "FFI signature requires a function name"),
            Self::MissingReturnType => write!(f, "FFI signature requires a return type"),
            Self::VoidArgument { index } => {
                write!(f, "FFI argument at index {index} cannot be void")
            }
            Self::EmptyName => write!(f, "FFI function name cannot be empty"),
            Self::Libffi(msg) => write!(f, "libffi error: {msg}"),
            Self::ArityMismatch { expected, got } => {
                write!(f, "FFI arity mismatch: expected {expected} args, got {got}")
            }
            Self::SymbolNotFound { name } => write!(f, "FFI symbol `{name}` not found in library"),
            Self::Unsupported(msg) => write!(f, "unsupported FFI signature: {msg}"),
        }
    }
}

impl std::error::Error for FfiError {}

impl FfiSignatureBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            ret: None,
        }
    }

    pub fn arg(mut self, ty: FfiType) -> Self {
        self.args.push(ty);
        self
    }

    pub fn args(mut self, args: impl IntoIterator<Item = FfiType>) -> Self {
        self.args.extend(args);
        self
    }

    pub fn ret(mut self, ty: FfiType) -> Self {
        self.ret = Some(ty);
        self
    }

    pub fn build(self) -> Result<FfiSignature, FfiError> {
        if self.name.is_empty() {
            return Err(FfiError::EmptyName);
        }
        for (index, ty) in self.args.iter().enumerate() {
            if *ty == FfiType::Void {
                return Err(FfiError::VoidArgument { index });
            }
        }
        let ret = self.ret.ok_or(FfiError::MissingReturnType)?;
        Ok(FfiSignature {
            name: self.name,
            args: self.args,
            ret,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_requires_return_type() {
        let err = FfiSignatureBuilder::new("f")
            .arg(FfiType::Int)
            .build()
            .unwrap_err();
        assert_eq!(err, FfiError::MissingReturnType);
    }

    #[test]
    fn builder_rejects_void_argument() {
        let err = FfiSignatureBuilder::new("f")
            .arg(FfiType::Void)
            .ret(FfiType::Int)
            .build()
            .unwrap_err();
        assert_eq!(err, FfiError::VoidArgument { index: 0 });
    }

    #[test]
    fn builder_produces_signature() {
        let sig = FfiSignatureBuilder::new("sum")
            .arg(FfiType::Int)
            .arg(FfiType::Int)
            .ret(FfiType::Int)
            .build()
            .unwrap();
        assert_eq!(sig.name, "sum");
        assert_eq!(sig.arity(), 2);
        assert_eq!(sig.ret, FfiType::Int);
    }
}
