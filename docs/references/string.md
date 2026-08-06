# `string` virtual module

String helpers are not auto-imported:

```coil
use string::{format, from_bytes, to_bytes};
```

| Export | Kind | Notes |
|--------|------|-------|
| `format` | Compiler intrinsic | `format("%i", value)` lowers to `FORMAT`; the first argument must be a string literal |
| `from_bytes` | Host native | Decode UTF-8 `Vec<byte>` to `Result<string, IoError>` |
| `to_bytes` | Host native | Encode `string` to UTF-8 `Vec<byte>` |

`format` checks literal `%` specifiers against argument types at compile time:

```coil
use string::format;

let s = format("%i-%s", 42, "x");
```

For stdout, combine `string` with `io`:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};

write_all(stdout(), to_bytes(format("%i", 42)));
```

`io::{from_bytes, to_bytes}` remain aliases for compatibility.

## Related

- [format](format.md)
- [io](io.md)
