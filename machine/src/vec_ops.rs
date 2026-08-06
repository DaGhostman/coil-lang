//! Host natives for builtin `Vec<T>` methods (pop/insert/remove/clear/…).
//!
//! Push uses the existing `ArrayPush` opcode; construction uses `MakeArray`.
//! Runtime representation is [`crate::memory::ObjArray`] (same as fixed
//! arrays until stack multi-slot lands).

use common::Value;

use crate::io::{alloc_option_none, alloc_option_some};
use crate::memory::{Heap, ObjArray, Object};

/// `Vec::with_capacity(n) -> Vec<T>` — empty growable array with reserved capacity.
pub fn host_vec_with_capacity(heap: &mut Heap, args: &[Value]) -> Value {
    let n = args.first().map(|v| v.as_int()).unwrap_or(0).max(0) as usize;
    let mut elements = Vec::with_capacity(n);
    elements.reserve(n);
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Value::from(obj.addr())
}

/// `v.capacity() -> int`
pub fn host_vec_capacity(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let cap = match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Array(gc)) => gc.as_ref().elements.capacity(),
        _ => 0,
    };
    Value::from(cap as i64)
}

/// `v.reserve(additional) -> ()` — ensure capacity for `len + additional`.
pub fn host_vec_reserve(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let extra = args.get(1).map(|v| v.as_int()).unwrap_or(0).max(0) as usize;
    if let Some(Object::Array(mut gc)) = heap.find_object_by_addr(handle.raw() as u64) {
        let old_bytes = gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
        gc.as_mut().elements.reserve(extra);
        let new_bytes = gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
        if old_bytes != new_bytes {
            heap.account_resize(old_bytes, new_bytes);
        }
    }
    Value::from(0i64)
}

/// `v.clear() -> ()`
pub fn host_vec_clear(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    if let Some(Object::Array(mut gc)) = heap.find_object_by_addr(handle.raw() as u64) {
        gc.as_mut().elements.clear();
    }
    Value::from(0i64)
}

/// `v.pop() -> Option<T>`
pub fn host_vec_pop(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Array(mut gc)) => match gc.as_mut().elements.pop() {
            Some(v) => alloc_option_some(heap, v),
            None => alloc_option_none(heap),
        },
        _ => alloc_option_none(heap),
    }
}

/// `v.insert(i, x) -> ()` — clamps out-of-range `i` to `len` (append).
pub fn host_vec_insert(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let index = args.get(1).map(|v| v.as_int()).unwrap_or(0);
    let value = args.get(2).copied().unwrap_or(Value::from(0i64));
    if let Some(Object::Array(mut gc)) = heap.find_object_by_addr(handle.raw() as u64) {
        let old_bytes = gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
        let len = gc.as_ref().elements.len();
        let i = if index < 0 {
            0usize
        } else if (index as usize) > len {
            len
        } else {
            index as usize
        };
        gc.as_mut().elements.insert(i, value);
        let new_bytes = gc.as_ref().elements.capacity() * std::mem::size_of::<Value>();
        if old_bytes != new_bytes {
            heap.account_resize(old_bytes, new_bytes);
        }
    }
    Value::from(0i64)
}

/// `v.remove(i) -> Option<T>`
pub fn host_vec_remove(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let index = args.get(1).map(|v| v.as_int()).unwrap_or(-1);
    match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Array(mut gc)) => {
            let len = gc.as_ref().elements.len();
            if index < 0 || (index as usize) >= len {
                alloc_option_none(heap)
            } else {
                let v = gc.as_mut().elements.remove(index as usize);
                alloc_option_some(heap, v)
            }
        }
        _ => alloc_option_none(heap),
    }
}

/// Copy a fixed array into a fresh growable vec (`Vec::from`).
pub fn host_vec_from_array(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let elements = match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Array(gc)) => gc.as_ref().elements.clone(),
        _ => Vec::new(),
    };
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Value::from(obj.addr())
}

/// Append-only HostInvoke wiring for Vec helpers.
pub const VEC_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    ("vec_with_capacity", 1, host_vec_with_capacity),
    ("vec_capacity", 1, host_vec_capacity),
    ("vec_reserve", 2, host_vec_reserve),
    ("vec_clear", 1, host_vec_clear),
    ("vec_pop", 1, host_vec_pop),
    ("vec_insert", 3, host_vec_insert),
    ("vec_remove", 2, host_vec_remove),
    ("vec_from_array", 1, host_vec_from_array),
];
