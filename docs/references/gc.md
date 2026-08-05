# `gc` module

`use gc::*;` — explicit GC pins and weak handles via HostInvoke.

| Surface | Types | Notes |
|---------|--------|--------|
| `Root<T>` | opaque type ctor | Strong pin; keeps `T` alive while the handle is reachable |
| `Weak<T>` | opaque type ctor | Non-rooting handle; does not keep `T` alive |
| `root` | `T -> Root<T>` | Allocate a pin around a value |
| `get` | `Root<T> -> T` | Read the pinned value; pin remains valid |
| `unroot` | `Root<T> -> T` | Take the value and clear the pin |
| `weak` | `T -> Weak<T>` | Allocate a non-rooting handle |
| `upgrade` | `Weak<T> -> Option<T>` | `Some` while the referent is live; `None` after collection |

## Semantics

- **`Root`** participates in mark-sweep: while a `Root` object is reachable from VM roots, its payload is marked.
- **`unroot`** clears the pin so a still-reachable `Root` shell no longer keeps the payload alive.
- **`Weak`** is not traced as a strong reference. After mark and before sweep, weaks whose heap referents are unmarked are cleared.
- **Immediates** (`int`, `bool`, …) under `Weak` always upgrade successfully (they are not heap objects).
- **`Root` / `Weak` are not thread-sendable.**

Typical FFI pattern: `root` a Coil buffer/callback before handing its address to C; hold `Weak` entries in Coil-side registries so maps do not extend lifetimes.

```coil
use gc::*;
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn main() {
    let r = root("alive");
    let w = weak(get(r));
    match upgrade(w) {
        Option::Some(s) => {
            write_all(stdout(), to_bytes(s));
        }
        Option::None => {
            write_all(stdout(), to_bytes("gone"));
        }
    }
}
```

---

## Related

- [FFI](ffi.md) — when C retains Coil pointers, pin with `root`
- [Modules](modules.md) — virtual module table
