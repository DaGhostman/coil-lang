//! Structural equality for heap values used by `EQ` / `NEQ`.
//!
//! Immediates and non-aggregate heap objects compare by machine word.
//! Arrays (and nested arrays) compare by length and element-wise recursion.
//! Strings compare by UTF-8 content (interned or not). Tuples compare
//! element-wise like arrays.

use std::collections::HashSet;

use common::Value;

use crate::memory::{Heap, Member, Object};

/// Deep / structural equality for VM values.
pub fn values_eq(heap: &Heap, a: Value, b: Value) -> bool {
    let mut seen = HashSet::new();
    values_eq_rec(heap, a, b, &mut seen)
}

fn values_eq_rec(heap: &Heap, a: Value, b: Value, seen: &mut HashSet<(u64, u64)>) -> bool {
    if a.raw() == b.raw() {
        return true;
    }
    let aa = a.raw() as u64;
    let bb = b.raw() as u64;
    if aa == 0 || bb == 0 {
        return false;
    }
    let key = if aa < bb { (aa, bb) } else { (bb, aa) };
    if !seen.insert(key) {
        // Already comparing this pair — treat as equal to break cycles.
        return true;
    }
    let Some(oa) = heap.find_object_by_addr(aa) else {
        return false;
    };
    let Some(ob) = heap.find_object_by_addr(bb) else {
        return false;
    };
    match (oa, ob) {
        (Object::Array(ga), Object::Array(gb)) => {
            let ea = &ga.as_ref().elements;
            let eb = &gb.as_ref().elements;
            if ea.len() != eb.len() {
                return false;
            }
            ea.iter()
                .zip(eb.iter())
                .all(|(x, y)| values_eq_rec(heap, *x, *y, seen))
        }
        (Object::Tuple(ga), Object::Tuple(gb)) => {
            let ea = &ga.as_ref().elements;
            let eb = &gb.as_ref().elements;
            if ea.len() != eb.len() {
                return false;
            }
            ea.iter()
                .zip(eb.iter())
                .all(|(x, y)| values_eq_rec(heap, *x, *y, seen))
        }
        (Object::String(ga), Object::String(gb)) => ga.as_ref().data == gb.as_ref().data,
        (Object::Boxed(ga), Object::Boxed(gb)) => match (&ga.as_ref().payload, &gb.as_ref().payload)
        {
            (Member::Value(va), Member::Value(vb)) => values_eq_rec(heap, *va, *vb, seen),
            (Member::Object(oa), Member::Object(ob)) => {
                values_eq_rec(heap, Value::from(oa.addr()), Value::from(ob.addr()), seen)
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{ObjArray, ObjString, ObjTuple};

    #[test]
    fn array_deep_eq_same_contents() {
        let mut heap = Heap::default();
        let (oa, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(1_i64), Value::from(2_i64)],
            },
            Object::Array,
        );
        let (ob, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(1_i64), Value::from(2_i64)],
            },
            Object::Array,
        );
        assert!(values_eq(
            &heap,
            Value::from(oa.addr()),
            Value::from(ob.addr())
        ));
    }

    #[test]
    fn array_deep_ne_different_len() {
        let mut heap = Heap::default();
        let (oa, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(1_i64)],
            },
            Object::Array,
        );
        let (ob, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(1_i64), Value::from(2_i64)],
            },
            Object::Array,
        );
        assert!(!values_eq(
            &heap,
            Value::from(oa.addr()),
            Value::from(ob.addr())
        ));
    }

    #[test]
    fn string_content_eq() {
        let mut heap = Heap::default();
        let (sa, _) = heap.alloc(ObjString::from("hi"), Object::String);
        let (sb, _) = heap.alloc(ObjString::from("hi"), Object::String);
        assert_ne!(sa.addr(), sb.addr());
        assert!(values_eq(
            &heap,
            Value::from(sa.addr()),
            Value::from(sb.addr())
        ));
    }

    #[test]
    fn tuple_deep_eq() {
        let mut heap = Heap::default();
        let (ta, _) = heap.alloc(
            ObjTuple {
                elements: vec![Value::from(3_i64), Value::from(4_i64)],
            },
            Object::Tuple,
        );
        let (tb, _) = heap.alloc(
            ObjTuple {
                elements: vec![Value::from(3_i64), Value::from(4_i64)],
            },
            Object::Tuple,
        );
        assert!(values_eq(
            &heap,
            Value::from(ta.addr()),
            Value::from(tb.addr())
        ));
    }
}
