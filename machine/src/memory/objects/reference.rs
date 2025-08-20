use std::{num::NonZero, ptr::NonNull};

use crate::{
    Object, ReferenceType,
    garbage::{Collectable, GcSized},
};

pub struct Reference(ReferenceType, usize);

impl From<Collectable<Object>> for Reference {
    fn from(value: Collectable<Object>) -> Self {
        Self(
            match value.as_ref() {
                Object::None => ReferenceType::None,
                Object::String(..) => ReferenceType::String,
                Object::Coroutine(..) => ReferenceType::Coroutine,
                Object::Reference(value) => return Self(value.as_ref().0, value.as_ref().1),
            },
            value.ptr().as_ptr() as usize,
        )
    }
}

impl GcSized for Reference {
    fn size(&self) -> usize {
        use std::mem::size_of_val;

        size_of_val(&self.0) + size_of_val(&self.1)
    }
}

impl Into<Collectable<Object>> for Reference {
    fn into(self) -> Collectable<Object> {
        let ptr = NonNull::without_provenance(NonZero::new(self.1).expect("Pointer is 0"));

        Collectable::from(ptr)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ref({:?}, 0x{:032x})", self.0, self.1)
    }
}
