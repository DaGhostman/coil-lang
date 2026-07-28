# `format`

### Syntax

```
format_expr ::= 'format' STRING (',' expr)*
```

`format` uses the same specifier rules as `print`, but returns the formatted `string` instead of writing to stdout.

```coil
let s = format "%i-%s", 42, "x";
print "%s", s; // 42-x
```

---

## Related

- [print](print.md)
