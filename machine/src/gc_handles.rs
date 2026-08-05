//! Host natives for virtual `gc` (`Root<T>` / `Weak<T>`).
//!
//! See [`GC_WIRING`] for pipeline `HostInvoke` registry names and arities.

use std::cell::Cell;

use common::Value;

use crate::io::{alloc_option_none, alloc_option_some};
use crate::memory::{Heap, Member, ObjRoot, ObjWeak, Object};

fn member_from_value(heap: &Heap, value: Value) -> Member {
    if !value.raw().is_null()
        && let Some(obj) = heap.find_object_by_addr(value.raw() as u64)
    {
        Member::Object(obj)
    } else {
        Member::Value(value)
    }
}

fn member_to_value(m: &Member) -> Value {
    match m {
        Member::Value(v) => *v,
        Member::Object(o) => Value::from(o.addr()),
    }
}

/// `gc::root(v) -> Root<T>` — allocate a strong pin around `v`.
pub fn host_gc_root(heap: &mut Heap, args: &[Value]) -> Value {
    let v = args.first().copied().unwrap_or(Value::from(0i64));
    let (obj, _) = heap.alloc(
        ObjRoot {
            payload: member_from_value(heap, v),
        },
        Object::Root,
    );
    Value::from(obj.addr())
}

/// `gc::get(root) -> T` — read the pinned value without releasing the pin.
pub fn host_gc_get(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Root(gc)) => member_to_value(&gc.as_ref().payload),
        _ => Value::from(0i64),
    }
}

/// `gc::unroot(root) -> T` — take the payload and clear the pin.
pub fn host_gc_unroot(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Root(gc)) => {
            let root = gc.payload_mut();
            let v = member_to_value(&root.payload);
            root.payload = Member::Value(Value::from(0i64));
            v
        }
        _ => Value::from(0i64),
    }
}

/// `gc::weak(v) -> Weak<T>` — non-rooting handle to `v`.
pub fn host_gc_weak(heap: &mut Heap, args: &[Value]) -> Value {
    let v = args.first().copied().unwrap_or(Value::from(0i64));
    let (obj, _) = heap.alloc(
        ObjWeak {
            target: Cell::new(v),
            cleared: Cell::new(false),
        },
        Object::Weak,
    );
    Value::from(obj.addr())
}

/// `gc::upgrade(weak) -> Option<T>` — `Some` while the referent is live.
pub fn host_gc_upgrade(heap: &mut Heap, args: &[Value]) -> Value {
    let handle = args.first().copied().unwrap_or(Value::from(0i64));
    let target = match heap.find_object_by_addr(handle.raw() as u64) {
        Some(Object::Weak(gc)) => {
            let weak = gc.as_ref();
            if weak.cleared.get() {
                None
            } else {
                Some(weak.target.get())
            }
        }
        _ => None,
    };
    match target {
        Some(v) => alloc_option_some(heap, v),
        None => alloc_option_none(heap),
    }
}

/// `gc::heap_bytes() -> int` — managed heap size in bytes (`Heap::size`).
pub fn host_gc_heap_bytes(heap: &mut Heap, _args: &[Value]) -> Value {
    Value::from(heap.size() as i64)
}

/// Registry name for [`host_gc_collect_stub`]; the VM HostInvoke path runs a
/// real stack-rooted collect when it sees this name.
pub const GC_COLLECT_NATIVE: &str = "gc_collect";

/// Stub for `gc::collect()` — the VM replaces this with a full collect.
///
/// Returns `0` if somehow invoked without the VM special-case (unit tests).
pub fn host_gc_collect_stub(_heap: &mut Heap, _args: &[Value]) -> Value {
    Value::from(0i64)
}

/// Registry names / arities for [`crate::host_natives::build_standard_host_natives`].
///
/// Append-only: keep prior ids stable.
pub const GC_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    ("gc_root", 1, host_gc_root),
    ("gc_unroot", 1, host_gc_unroot),
    ("gc_get", 1, host_gc_get),
    ("gc_weak", 1, host_gc_weak),
    ("gc_upgrade", 1, host_gc_upgrade),
    ("gc_heap_bytes", 0, host_gc_heap_bytes),
    (GC_COLLECT_NATIVE, 0, host_gc_collect_stub),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn intern(heap: &mut Heap, s: &str) -> Value {
        let gc = heap.intern(s.to_string());
        Value::from(gc.as_ptr() as *mut u8 as u64)
    }

    fn force_collect(heap: &mut Heap, keep: &[Value]) {
        let mut roots: Vec<u64> = keep
            .iter()
            .map(|v| v.raw() as u64)
            .filter(|&a| a != 0 && heap.find_object_by_addr(a).is_some())
            .collect();
        // Also keep Root/Weak handles themselves when passed in `keep`.
        heap.trace(&roots);
        let mut gray = Vec::new();
        let mut root_objects = Vec::new();
        let mut current = heap.head_for_lookup();
        while let Some(reference) = current {
            if reference.is_marked() {
                root_objects.push(reference);
            }
            current = reference.get_next();
        }
        for root in &root_objects {
            root.mark_references(&mut gray);
            if let Object::Array(gc) = root {
                for v in &gc.as_ref().elements {
                    let addr = v.raw() as u64;
                    if let Some(obj) = heap.find_object_by_addr(addr) {
                        obj.mark(&mut gray);
                    }
                }
            }
        }
        while let Some(obj) = gray.pop() {
            obj.mark_references(&mut gray);
            if let Object::Array(gc) = obj {
                for v in &gc.as_ref().elements {
                    let addr = v.raw() as u64;
                    if let Some(o) = heap.find_object_by_addr(addr) {
                        o.mark(&mut gray);
                    }
                }
            }
        }
        heap.clear_dead_weaks();
        unsafe { heap.sweep() };
        let _ = &mut roots;
    }

    #[test]
    fn root_keeps_payload_alive() {
        let mut heap = Heap::default();
        let s = intern(&mut heap, "pinned");
        let root = host_gc_root(&mut heap, &[s]);
        // Drop the only direct reference to the string; Root should keep it.
        force_collect(&mut heap, &[root]);
        let got = host_gc_get(&mut heap, &[root]);
        match heap.find_object_by_addr(got.raw() as u64) {
            Some(Object::String(gc)) => assert_eq!(gc.as_ref().data, "pinned"),
            _ => panic!("expected rooted string to survive"),
        }
    }

    #[test]
    fn unroot_allows_collection() {
        let mut heap = Heap::default();
        let s = intern(&mut heap, "gone");
        let root = host_gc_root(&mut heap, &[s]);
        let taken = host_gc_unroot(&mut heap, &[root]);
        assert_eq!(
            heap.find_object_by_addr(taken.raw() as u64)
                .map(|o| matches!(o, Object::String(_))),
            Some(true)
        );
        // Neither root shell nor string kept — both should die.
        force_collect(&mut heap, &[]);
        assert!(heap.find_object_by_addr(taken.raw() as u64).is_none());
        assert!(heap.find_object_by_addr(root.raw() as u64).is_none());
    }

    #[test]
    fn weak_does_not_keep_alive_and_upgrade_clears() {
        let mut heap = Heap::default();
        let s = intern(&mut heap, "ephemeral");
        let w = host_gc_weak(&mut heap, &[s]);
        match host_gc_upgrade(&mut heap, &[w]) {
            some => {
                // Option::Some
                match heap.find_object_by_addr(some.raw() as u64) {
                    Some(Object::Enum(gc)) => assert_eq!(gc.as_ref().tag, 1),
                    _ => panic!("expected Option"),
                }
            }
        }
        force_collect(&mut heap, &[w]);
        let up = host_gc_upgrade(&mut heap, &[w]);
        match heap.find_object_by_addr(up.raw() as u64) {
            Some(Object::Enum(gc)) => assert_eq!(gc.as_ref().tag, 0, "expected None after collect"),
            _ => panic!("expected Option::None"),
        }
    }

    #[test]
    fn root_and_weak_together() {
        let mut heap = Heap::default();
        let s = intern(&mut heap, "both");
        let root = host_gc_root(&mut heap, &[s]);
        let w = host_gc_weak(&mut heap, &[s]);
        force_collect(&mut heap, &[root, w]);
        let up = host_gc_upgrade(&mut heap, &[w]);
        match heap.find_object_by_addr(up.raw() as u64) {
            Some(Object::Enum(gc)) => assert_eq!(gc.as_ref().tag, 1),
            _ => panic!("expected Some while Root lives"),
        }
    }

    #[test]
    fn weak_of_immediate_always_upgrades() {
        let mut heap = Heap::default();
        let w = host_gc_weak(&mut heap, &[Value::from(42i64)]);
        force_collect(&mut heap, &[w]);
        let up = host_gc_upgrade(&mut heap, &[w]);
        match heap.find_object_by_addr(up.raw() as u64) {
            Some(Object::Enum(gc)) => {
                assert_eq!(gc.as_ref().tag, 1);
                assert_eq!(member_to_value(&gc.as_ref().payload[0]).as_int(), 42);
            }
            _ => panic!("expected Some(42)"),
        }
    }

    #[test]
    fn heap_bytes_tracks_alloc() {
        let mut heap = Heap::default();
        let before = host_gc_heap_bytes(&mut heap, &[]).as_int();
        let _ = host_gc_root(&mut heap, &[Value::from(1i64)]);
        let after = host_gc_heap_bytes(&mut heap, &[]).as_int();
        assert!(after > before);
    }
}
