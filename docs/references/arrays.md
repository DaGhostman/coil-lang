# Array append (`arr[] =`) and `len`

Append with empty index assignment; query length with `len`.

```coil
arr[] = value   // append (assignment target only)
len(arr)
```

| Form | Argument types | Returns | Behavior |
|------|----------------|---------|----------|
| `arr[] = v` | `[T]`, `T` | `[T]` (discarded in statement form) | Appends in place; promotes fixed `[T; N]` bindings to dynamic `[T]` |
| `len` | `[T]` | `int` | Current runtime length |

Empty `arr[]` is only valid as an assignment target — using it as an rvalue is a compile error.

```coil
let a = [1, 2];
a[] = 3;
print "%i", len(a); // 3
print "%i", a[2];  // 3
```

---

## Related

- [Syntax](syntax.md)
- [Types](types.md)
