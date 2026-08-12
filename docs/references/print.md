# `print` removal

The `print` statement has been removed. Write to stdout through the virtual `io` module and convert strings to bytes through the virtual `string` module. `write_all` is a [coil-stdlib](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/io.md) adapter.

Old:

```coil
print "%i", x;
```

New:

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

write_all(stdout(), to_bytes(format("%i", x)));
```

Literal-only output does not need `format`:

```coil
use io::{stdout};
use io::sync::{write_all};
use string::to_bytes;

write_all(stdout(), to_bytes("hello"));
```

### Format specifiers

The typechecker validates specifiers against arguments when the format string is a compile-time literal.

| Specifier | Argument type | Output |
|-----------|---------------|--------|
| `%i` | `int` | Signed decimal integer |
| `%f` | `float` | Float (debug-style formatting) |
| `%s` | `string` | String contents |
| `%z` | `bool` | `true` or `false` |
| `%v` | `T: Show` | `show(value)` then inserted as a string |
| `%b` | `int` | Binary representation (VM-specific) |
| `%x` | `int` | Hex representation (VM-specific) |
| `%u` | `int` | Unsigned-style address rendering |
| `%p` | `int` | Pointer-style hex |
| `%%` | *(none)* | Literal `%` |

**Not supported:** `%d` (rejected by typechecker — use `%i`).

`%v` works for open type parameters when the enclosing function has a `Show` bound. Concrete `%i`/`%f`/`%s`/`%z` on an unresolved type variable are rejected (help text recommends `%v`).

---

`FORMAT` still builds formatted strings for `string::format`. Stdout writes are ordinary `io::write_all(stdout(), bytes)` calls.

## Related

- [format](format.md)
- [string](string.md)
- [io](io.md)
- [Tutorial 01](../manual/tutorial/01-basics.md)
