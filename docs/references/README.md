# References

Lookup docs for language constructs and compiler-provided APIs. For a guided introduction, start with the [manual](../manual/getting-started.md).

Compiler builtins live in **virtual modules** (not `.hy` files). Every file gets an implicit `use prelude::*; use prelude::ops::*; use prelude::test::*; use prelude::math::*;`. FFI, `io`, `string`, `thread`, `time`, `env`, `crypto`, `regex`, and `gc` require an explicit `use`.

## Language

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Grammar overview, declarations, expressions |
| [Types](types.md) | Type system, aliases, aggregates, generics |
| [Operators](operators.md) | Arithmetic, comparison, logical, field access |
| [Keywords](keywords.md) | Reserved words and constructs |
| [Modules](modules.md) | Namespace rules, `use` resolution |
| [Project config](project-config.md) | `coil.toml` manifest format |
| [Error codes](error-codes.md) | Stable `E####` diagnostic codes |

## Built-ins and virtual modules

| Document | Kind | Purpose |
|----------|------|---------|
| [Option / Result](option-result.md) | Prelude enums | Built-in sum types |
| [print](print.md) | Migration note | Removed statement; use `io` + `string` |
| [format](format.md) | Intrinsic | `string::format(...)` builds a formatted string |
| [string](string.md) | Virtual module | `format` / UTF-8 byte conversions |
| [arrays](arrays.md) | Expression | `arr[] =` append and `len` |
| [math](math.md) | Prelude | `dot` / `matmul` / `cross` / `Matrix` |
| [FFI](ffi.md) | Virtual module | `dload` / `declare` / `invoke` / `extern` |
| [done](done.md) | Expression | Coroutine finished? |
| [io](io.md) | Virtual module | Non-blocking streams, TCP, UDP |
| [io::fs](io-fs.md) | Virtual module | Path / metadata helpers |
| [Iterator](iterator.md) | Prelude traits | `for x in` protocol |
| [assert](assert.md) | Prelude test | `assert(cond[, msg]) → Result` |
| [test harness](test-harness.md) | CLI | `test("…")` / `#[test]` |
| [panic](panic.md) | Keyword | Abort with a message |
| [casts](casts.md) | Expression | `expr as T` |
| [time](time.md) | Virtual module | Timestamps, sleep |
| [env](env.md) | Virtual module | Args, env vars, `exec` |
| [crypto](crypto.md) | Virtual module | Hashes, AEAD, keys |
| [regex](regex.md) | Virtual module | PCRE2 |
| [gc](gc.md) | Virtual module | `Root` / `Weak` pins |
| [ord / char](ord-char.md) | Prelude | Single-byte string ↔ `byte` |
| [host natives](host-natives.md) | Embedder API | Rust closures via `HostInvoke` |
| [What is NOT a builtin](not-builtins.md) | Scope | Gaps vs builtins; see also [`stdlib/`](../../stdlib/) userland |

Userland packages under `stdlib/` (include in `[module].roots`): `bytes`, `text`,
`collections`, `num`, `random`, `path`, `json`, `io::sync` / `io::file`, `http`.
See [`stdlib/README.md`](../../stdlib/README.md).

## Related

- [Manual](../manual/getting-started.md) — tutorials and examples
- [Internals](../internals/README.md) — pipeline, VM, debug info
