# `gc` module

`use gc::{root, weak, upgrade, collect, heap_bytes};` — explicit GC pins, weak handles, heap stats, and manual collection via HostInvoke.

| Surface | Types | Notes |
|---------|--------|--------|
| `Root<T>` | opaque type ctor | Strong pin; keeps `T` alive while the handle is reachable |
| `Weak<T>` | opaque type ctor | Non-rooting handle; does not keep `T` alive |
| `root` | `T -> Root<T>` | Allocate a pin around a value |
| `get` | `Root<T> -> T` | Read the pinned value; pin remains valid |
| `unroot` | `Root<T> -> T` | Take the value and clear the pin |
| `weak` | `T -> Weak<T>` | Allocate a non-rooting handle |
| `upgrade` | `Weak<T> -> Option<T>` | `Some` while the referent is live; `None` after collection |
| `heap_bytes` | `() -> int` | Managed heap size in bytes (`Heap::size`) |
| `collect` | `() -> int` | Force a full mark-sweep; returns bytes freed |

## Semantics

- **`Root`** participates in mark-sweep: while a `Root` object is reachable from VM roots, its payload is marked.
- **`unroot`** clears the pin so a still-reachable `Root` shell no longer keeps the payload alive.
- **`Weak`** is not traced as a strong reference. After mark and before sweep, weaks whose heap referents are unmarked are cleared.
- **Immediates** (`int`, `bool`, …) under `Weak` always upgrade successfully (they are not heap objects).
- **`Root` / `Weak` are not thread-sendable.**
- **`heap_bytes`** reports VM-managed heap accounting only — not process RSS (native libs, stacks, Rust allocators sit outside it).
- **`collect`** roots the operand stack and suspended coroutines the same way automatic GC does.

Typical FFI pattern: `root` a Coil buffer/callback before handing its address to C; hold `Weak` entries in Coil-side registries so maps do not extend lifetimes.

```coil
use gc::{collect, get, root, unroot, upgrade, weak};
use io::{stdout};
use io::sync::{write_all};
use string::{to_bytes};

fn ephemeral_weak() {
    let r = root([1, 2, 3]);
    let w = weak(get(r));
    let dropped = unroot(r);
    dropped = [];
    return w;
}

fn main() {
    let w = ephemeral_weak();
    collect();
    let label = match upgrade(w) {
        Option::Some(_) => "some",
        Option::None => "none",
    };
    write_all(stdout(), to_bytes(label));
}
```

Drop all strong refs (including frame locals / operand temps that still name the
value) before `collect`, or return only the `Weak` from a helper as above.

---

## Related

- [FFI](ffi.md) — when C retains Coil pointers, pin with `root`
- [Modules](modules.md) — virtual module table
