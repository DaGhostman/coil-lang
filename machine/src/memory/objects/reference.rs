use std::{num::NonZero, ptr::NonNull};

use crate::{
    garbage::{Collectable, GcSized}, Object, ObjectType, 
};

#[derive(Clone)]
pub struct Reference(ObjectType, usize);

impl From<Object> for Reference {
    fn from(value: Object) -> Self {
        Self(
            match value {
                Object::None => ObjectType::None,
                Object::String(..) => ObjectType::String,
                Object::Coroutine(..) => ObjectType::Coroutine,
                Object::Reference(value) => return Self(value.0, value.1),
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

impl From<Reference> for Collectable<Object> {
    fn from(val: Reference) -> Self {
        let ptr = NonNull::without_provenance(NonZero::new(val.1).expect("Pointer is 0"));

        Collectable::from(ptr)
    }
}

#[cfg(debug_assertions)]
impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ref({:?}, 0x{:032x})", self.0, self.1)
    }
}
