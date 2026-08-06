# `string::format`

### Syntax

```
format_call ::= 'format' '(' STRING (',' expr)* ')'
```

`format` is exported by the virtual `string` module. It is a compiler intrinsic, not a `HostInvoke`, and lowers to the `FORMAT` opcode.

The first argument must be a string literal so the typechecker can validate each `%` specifier against the corresponding argument.

```coil
use string::format;

let s = format("%i-%s", 42, "x");
```

Use `io` and `to_bytes` to write the formatted string:

```coil
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

write_all(stdout(), to_bytes(format("%s", "hello")));
```

---

## Related

- [string](string.md)
- [print migration note](print.md)
