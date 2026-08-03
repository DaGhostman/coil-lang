# Array append (`arr[] =`) and `len`

Append with empty index assignment; query length with `len`.

```coil
arr[] = value   // append (assignment target only)
len(value)
```

| Form | Argument types | Returns | Behavior |
|------|----------------|---------|----------|
| `arr[] = v` | `[T]`, `T` | `[T]` (discarded in statement form) | Appends in place; promotes fixed `[T; N]` bindings to dynamic `[T]` |
| `len` | `[T]`, `string`, tuple, dict, or `T: Length` | `int` | Structural length, or `Length::len` for custom types |

`len` of a string, array, tuple, or dict **literal** (and of fixed-size array/tuple types) folds to a compile-time integer when the length is statically known. Custom types implement `Length`:

```coil
impl Length for Pair {
    fn len(Pair p) -> int { return 2; }
}
```

Empty `arr[]` is only valid as an assignment target — using it as an rvalue is a compile error.

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
let a = [1, 2];
a[] = 3;
write_all(stdout(), to_bytes(format("%i", len(a)))); // 3
write_all(stdout(), to_bytes(format("%i", a[2])));  // 3
write_all(stdout(), to_bytes(format("%i", len("foo")))); // 3 (folded)
```

---

## Related

- [Syntax](syntax.md)
- [Types](types.md)
