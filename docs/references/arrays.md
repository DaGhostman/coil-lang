# Arrays and `Vec`

coil splits fixed-size arrays from growable vectors (Rust-style).

## Fixed arrays — `[T; N]`

Homogeneous, fixed length. `N` is part of the type and is **inferred** from
literals when possible; otherwise it must be written explicitly.

```coil
let a = [1, 2, 3];          // [int; 3]
let b: [int; 3] = [0, 0, 0];
a[1] = 9;                   // element assign only
```

| Rule | Behavior |
|------|----------|
| Literal `[e, …]` | Infers `[T; N]` with `N =` element count |
| Annotation `[T]` | **Error** — use `[T; N]` or `Vec<T>` |
| Empty `[]` | Only under `Vec<T>` or `[T; 0]` |
| Growth | **Forbidden** — no `arr[] =`; use `Vec` |

Locals of type `[T; N]` are laid out as **N consecutive frame slots** (stack).
Escaping into a single-value context (call, return, store into a heap object)
boxes into a non-growable heap array.

`len(a)` folds to `N` when the length is static. Element-wise zip / LA helpers
still require fixed lengths.

## `Vec<T>` — growable heap vector

```coil
let v: Vec<int> = Vec::new();
v.push(1);
v.push(2);
let x = v[0];
v[0] = 7;
match v.pop() {
    Option::Some(n) => { /* … */ },
    Option::None => { /* … */ },
};
```

### Methods

| Method | Notes |
|--------|--------|
| `Vec::new()` | Empty vector |
| `Vec::with_capacity(n)` | Empty with reserved capacity |
| `Vec::from(arr)` | Copy a fixed `[T; N]` into a `Vec` |
| `v.push(x)` | Append |
| `v.pop()` | `Option<T>` |
| `v.insert(i, x)` | Insert at index (clamped to `len`) |
| `v.remove(i)` | `Option<T>` |
| `v.clear()` | Drop all elements |
| `v.reserve(n)` | Ensure capacity for `len + n` |
| `v.capacity()` / `v.len()` | Ints |
| `v[i]` / `v[i] = x` | Index get/set |

Rest parameters `T... xs` pack into `Vec<T>`. Spread accepts both `[T; N]` and
`Vec<T>`.

IO buffers (`to_bytes`, `read`/`write`) use `Vec<byte>`.

---

## Related

- [Types](types.md)
- [Syntax](syntax.md)
- Example: `examples/vec.hy`
