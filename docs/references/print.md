# `print`

### Syntax

```
print_stmt ::= 'print' STRING (',' expr)* ';'
```

### Forms

| Form | Example | Behavior |
|------|---------|----------|
| Literal only | `print "hello";` | Writes `hello` |
| Format + args | `print "%i", x;` | Interpolates specifiers |
| Multiple args | `print "%i %s", n, name;` | One specifier per arg, left to right |

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

### Examples

```coil
print "plain text";
print "%i", 42;
print "%s %z", "ok", true;
print "100%% complete";   // literal percent via %%
```

### Runtime pipeline

1. If specifiers present: `FORMAT` builds a new string on the heap.
2. `PRINT` pops the string and writes to stdout (or a redirected writer in tests).

See [Tutorial 01](../manual/tutorial/01-basics.md) for introductory usage.

---

Internal: the `FORMAT` opcode powers both `print` and the `format` expression.

## Related

- [format](format.md)
- [Tutorial 01](../manual/tutorial/01-basics.md)
