//! Structural equality for heap values used by `EQ` / `NEQ`.
//!
//! Immediates and non-aggregate heap objects compare by machine word.
//! Arrays (and nested arrays) compare by length and element-wise recursion.
//! Strings compare by UTF-8 content (interned or not). Tuples compare
//! element-wise like arrays.
//!
//! Cyclic graphs use a bijection of addresses already assumed equal: revisiting
//! `a` must pair with the same `b` (and vice versa). A 1-cycle is therefore not
//! equal to a 2-cycle.

use std::collections::HashMap;

use common::Value;

use crate::memory::{Heap, Member, Object};

/// Deep / structural equality for VM values.
pub fn values_eq(heap: &Heap, a: Value, b: Value) -> bool {
    let mut fwd = HashMap::new();
    let mut rev = HashMap::new();
    values_eq_rec(heap, a, b, &mut fwd, &mut rev)
}

fn values_eq_rec(
    heap: &Heap,
    a: Value,
    b: Value,
    fwd: &mut HashMap<u64, u64>,
    rev: &mut HashMap<u64, u64>,
) -> bool {
    if a.raw() == b.raw() {
        return true;
    }
    let aa = a.raw() as u64;
    let bb = b.raw() as u64;
    if aa == 0 || bb == 0 {
        return false;
    }
    if let Some(&mapped) = fwd.get(&aa) {
        return mapped == bb;
    }
    if let Some(&mapped) = rev.get(&bb) {
        return mapped == aa;
    }
    fwd.insert(aa, bb);
    rev.insert(bb, aa);
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
                .all(|(x, y)| values_eq_rec(heap, *x, *y, fwd, rev))
        }
        (Object::Tuple(ga), Object::Tuple(gb)) => {
            let ea = &ga.as_ref().elements;
            let eb = &gb.as_ref().elements;
            if ea.len() != eb.len() {
                return false;
            }
            ea.iter()
                .zip(eb.iter())
                .all(|(x, y)| values_eq_rec(heap, *x, *y, fwd, rev))
        }
        (Object::String(ga), Object::String(gb)) => ga.as_ref().data == gb.as_ref().data,
        (Object::Boxed(ga), Object::Boxed(gb)) => match (&ga.as_ref().payload, &gb.as_ref().payload)
        {
            (Member::Value(va), Member::Value(vb)) => values_eq_rec(heap, *va, *vb, fwd, rev),
            (Member::Object(oa), Member::Object(ob)) => {
                values_eq_rec(heap, Value::from(oa.addr()), Value::from(ob.addr()), fwd, rev)
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

    fn set_array_elem(heap: &Heap, addr: u64, index: usize, value: Value) {
        let Some(Object::Array(gc)) = heap.find_object_by_addr(addr) else {
            panic!("expected array at {addr:#x}");
        };
        gc.payload_mut().elements[index] = value;
    }

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

    #[test]
    fn self_loop_arrays_are_equal() {
        let mut heap = Heap::default();
        let (oa, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (ob, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        set_array_elem(&heap, oa.addr(), 0, Value::from(oa.addr()));
        set_array_elem(&heap, ob.addr(), 0, Value::from(ob.addr()));
        assert!(values_eq(
            &heap,
            Value::from(oa.addr()),
            Value::from(ob.addr())
        ));
    }

    #[test]
    fn one_cycle_ne_two_cycle() {
        // a = [a]  vs  b = [c], c = [b]
        let mut heap = Heap::default();
        let (oa, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (ob, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (oc, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        set_array_elem(&heap, oa.addr(), 0, Value::from(oa.addr()));
        set_array_elem(&heap, ob.addr(), 0, Value::from(oc.addr()));
        set_array_elem(&heap, oc.addr(), 0, Value::from(ob.addr()));
        assert!(!values_eq(
            &heap,
            Value::from(oa.addr()),
            Value::from(ob.addr())
        ));
    }

    #[test]
    fn matching_two_cycles_are_equal() {
        // a=[b], b=[a]  vs  c=[d], d=[c]
        let mut heap = Heap::default();
        let (oa, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (ob, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (oc, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        let (od, _) = heap.alloc(
            ObjArray {
                elements: vec![Value::from(0_i64)],
            },
            Object::Array,
        );
        set_array_elem(&heap, oa.addr(), 0, Value::from(ob.addr()));
        set_array_elem(&heap, ob.addr(), 0, Value::from(oa.addr()));
        set_array_elem(&heap, oc.addr(), 0, Value::from(od.addr()));
        set_array_elem(&heap, od.addr(), 0, Value::from(oc.addr()));
        assert!(values_eq(
            &heap,
            Value::from(oa.addr()),
            Value::from(oc.addr())
        ));
    }
}
