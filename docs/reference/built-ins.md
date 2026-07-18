# Built-ins reference

Built-in facilities provided by the language runtime and compiler — not ordinary user-defined functions in a standard library.

zero-script does **not** yet ship a general-purpose stdlib (no `map`, `filter`, file I/O modules, etc.). What exists today is I/O, FFI, and host-embedder hooks.

---

## Overview

| Builtin | Kind | Purpose |
|---------|------|---------|
| `print` | Statement | Write to stdout |
| `format` | Expression | Build a formatted string |
| `push` | Expression | Append to an array in place and return the array |
| `len` | Expression | Return an array length |
| `ffi::{dload,declare,invoke}` | Virtual module | Runtime FFI (requires `use ffi::*`) |
| `ffi::types::{Int,…}` | Virtual module | FFI type-tag constructors (requires `use ffi::types::*`) |
| `io::{open,read,write,…}` | Virtual module | Non-blocking streams + sync adapters (requires `use io::*`) |
| `done` | Expression | `true` if a coroutine handle is finished |
| `prelude::{Option,Result}` | Virtual module | Auto-imported sum types |
| `prelude::ops::{Add,Eq,…}` | Virtual module | Auto-imported operator traits |
| `prelude::test::assert` | Virtual module | Auto-imported; `assert(cond[, msg]) → Result<(), string>` |
| `panic` | Keyword | Abort with a string message (exit code 1) |
| Host natives | Embedder API | Rust closures from `Pipeline::register_host_native` |

Compiler builtins live in **virtual modules** (not `.0s` files). Every file gets an implicit `use prelude::*; use prelude::ops::*; use prelude::test::*;`. FFI and **`io`** are **not** auto-imported — write `use ffi::*;` / `use io::*;` before using those APIs.

---

## `Option` and `Result`

Pre-registered enums with fixed tags, exported from the virtual `prelude` module (auto-imported into every file):

| Enum | Variants | Tags | Canonical path |
|------|----------|------|----------------|
| `Option` | `None`, `Some(T)` | 0, 1 | `prelude::Option` |
| `Result` | `Ok(T)`, `Err(E)` | 0, 1 | `prelude::Result` |

Bare `Option::Some(…)` works because of the implicit prelude. To redefine a prelude name, first free the short binding (`use prelude::Option as PreludeOption;`) then declare your own.

Use constructors / `match` as usual, plus `raise`, `?`, `??`, and `?.` — see [Tutorial: Error handling](../tutorial/09-error-handling.md).

Internal: the `FORMAT` opcode powers both `print` and the `format` expression.

---

## `print`

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

```0s
print "plain text";
print "%i", 42;
print "%s %z", "ok", true;
print "100%% complete";   // literal percent via %%
```

### Runtime pipeline

1. If specifiers present: `FORMAT` builds a new string on the heap.
2. `PRINT` pops the string and writes to stdout (or a redirected writer in tests).

See [Tutorial 01](../tutorial/01-basics.md) for introductory usage.

---

## `push` and `len`

Array helpers compiled as built-in calls.

```0s
push(arr, value)
len(arr)
```

| Builtin | Argument types | Returns | Behavior |
|---------|----------------|---------|----------|
| `push` | `[T]`, `T` | `[T]` | Appends to the heap array in place and returns the same array for chaining |
| `len` | `[T]` | `int` | Returns the current runtime length |

`push` promotes a fixed-length array binding to dynamic array type for later checks, so indexing a newly appended literal position is accepted after the push:

```0s
let a = [1, 2];
push(a, 3);
print "%i", len(a); // 3
print "%i", a[2];  // 3
```

---

## `format`

### Syntax

```
format_expr ::= 'format' STRING (',' expr)*
```

`format` uses the same specifier rules as `print`, but returns the formatted `string` instead of writing to stdout.

```0s
let s = format "%i-%s", 42, "x";
print "%s", s; // 42-x
```

---

## `dload` / `declare` / `invoke` (`ffi`)

Runtime FFI callables are exports of the virtual `ffi` module. They are **not** keywords and are **not** in scope until you import them:

```0s
use ffi::*;
use ffi::types::*;
```

Or import individually: `use ffi::dload;`, `use ffi::declare;`, `use ffi::invoke;`.

### `dload`

Load a native shared library at runtime.

```0s
dload(path_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `path_expr` | `string` | Path or name passed to `dlopen` |

Returns a library handle as `int` (heap library object address), or `-1` on failure.

```0s
use ffi::*;
let lib = dload("libsum.so");
```

Notes:

- Requires libffi-enabled build.
- Prefer full paths when cwd is unpredictable.
- Same mechanism as the string in `extern "..." { ... }` blocks (`extern` does **not** require `use ffi::*`).

---

## `done`

Test whether a coroutine handle has finished.

### Syntax

```0s
done(handle_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `handle_expr` | `coroutine<Y, S>` | Handle from calling an `async fn` |

### Returns

`bool` — `true` after the coroutine body has returned (or fallen off the end); `false` while still suspended at a `yield` or before the first `resume`.

### Example

```0s
let h = counter();
print "%z", done(h); // false
resume h;
resume h;            // completes
print "%z", done(h); // true
```

---

### `declare`

Register a C function signature in a loaded library.

```0s
declare(lib, name, (arg_types...), ret_type)
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Handle from `dload` |
| `name` | `string` | Symbol name for `dlsym` |
| `(arg_types...)` | Tuple of FFI tags | One tag per parameter |
| `ret_type` | FFI tag | Return type (`void` allowed) |

Returns a function id (`int`), or `-1` if symbol missing or libffi rejects signature.

### FFI type tags (`ffi::types`)

Tag constructors live in the virtual `ffi::types` module. After `use ffi::types::*;`, write bare `Int`, `Ptr`, `Callback`, …:

```0s
use ffi::*;
use ffi::types::*;

declare(lib, "f", (Int, String), Int);
declare(lib, "g", (int, float), void);   // bare lowercase names still work
declare(lib, "h", (ffi::types::Ptr,), Int); // qualified path needs no glob
```

| Tag | Meaning |
|-----|---------|
| `int` / `Int` | 64-bit integer |
| `float` / `Float` | 64-bit float |
| `string` / `String` | C string |
| `void` / `Void` | No return value only |
| `Ptr` / `Callback` / … | See [FFI tutorial](../tutorial/07-ffi.md) |

`void` cannot appear as an argument type. There is no global bare `FFIType` name — import `ffi::types` (or use the qualified `ffi::types::Int` path).

---

### `invoke`

Call a function registered with `declare`.

```0s
invoke(lib, fn_id, (args...))
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Same library handle |
| `fn_id` | `int` | Id from `declare` |
| `(args...)` | Tuple of values | Must match declared arity and types |

Returns a value per the declared return type. `void` functions push nothing meaningful — do not rely on a return value.

```0s
print "%i", invoke(lib, sum_id, (40, 2));
```

`invoke` returns the type recorded from the matching `declare(..., ret)` (or `unit` for `void`). Bind the `declare` result with `let id = declare(...)` so the side table can refine later `invoke` calls.

---

## Compile-time FFI (`extern` blocks)

Not separate builtins — the compiler lowers extern declarations to `dload` / `declare` / `invoke` sequences. User code calls look like normal functions:

```0s
extern "libc.so.6" {
    fn strlen(string s) -> int;
}

fn main() {
    print "%i", strlen("hello");
}
```

See [FFI tutorial](../tutorial/07-ffi.md).

---

## `io` virtual module

Non-blocking file / stdio / TCP streams. **Not** auto-imported:

```0s
use io::*;
```

| Export | Kind | Notes |
|--------|------|-------|
| `Stream` | Opaque type | Heap handle; closed on GC drop |
| `IoError` | Builtin enum | `WouldBlock`, `NotFound`, `PermissionDenied`, `AlreadyClosed`, `InvalidInput`, `Other` |
| `Read` / `Write` | Typeclasses | `impl` for `Stream`; methods = free functions |
| `stdin` / `stdout` / `stderr` | `() -> Stream` | Dup'd fds |
| `open` / `close` / `read` / `write` | L0 | Never busy-spin; `read` → `Result<Option<int>, IoError>` (`None` = EOF) |
| `read_exact` / `read_to_end` / `write_all` | Sync adapters | May block in the host via `poll` |
| `from_bytes` / `to_bytes` | Text | UTF-8 `[byte] ↔ string` (`from_bytes` → `Result<string, IoError>`) |
| `tcp_connect` / `tcp_listen` / `tcp_accept` / `tcp_accept_wait` | TCP | Same `Stream` contract |
| `udp_bind` / `udp_connect` / `udp_send_to` / `udp_recv_from` / `udp_recv_from_wait` / `udp_local_port` | UDP | Datagram sockets; `recv_from` → `(nbytes, host, port)` |

Buffers are **`[byte]`**. Use `from_bytes` / `to_bytes` for text. `print` still uses the `PRINT` opcode (not `stdout`). No HTTP in the VM — userland only later.

See [Tutorial 10 — IO streams](../tutorial/10-io-streams.md) and `examples/io_*.0s`.

---

## What is NOT a builtin

There is **no general standard library** yet. The following are **not** built-in — you must provide your own functions or FFI:

| Category | Examples |
|----------|----------|
| Collections API | `sort`, iterators (`push` / `len` are builtins) |
| String ops | slice, trim (concat via `+` / `format`; UTF-8 via `io::from_bytes` / `to_bytes`) |
| Math | `sin`, `sqrt`, `random` |
| High-level file helpers | path utilities beyond `io::open` / `read_to_end` / `write_all` |
| HTTP / TLS | Not in the VM (use userland on top of `io` TCP later) |
| Concurrency | OS threads (stackful coroutines via `async` / `yield` / `resume` / `done` are supported) |
| Memory | `alloc`, `free` |

Use **`io`** for streams, **FFI** for C libraries, or **host natives** when embedding the VM in Rust.

---

## Host embedder API

Advanced: register Rust closures callable from bytecode without a `.so` file.

### Rust API

```rust
use compiler::pipeline::Pipeline;
use machine::ffi::{FfiSignature, FfiSignatureBuilder};
use machine::memory::{FfiType, Heap};
use common::Value;

let mut pipeline = Pipeline::default();

let sig = FfiSignatureBuilder::new("my_add")
    .arg(FfiType::Int)
    .arg(FfiType::Int)
    .ret(FfiType::Int)
    .build()
    .unwrap();

pipeline.register_host_native(sig, |heap: &mut Heap, args: &[Value]| {
    let sum = args[0].as_int() + args[1].as_int();
    Ok(Some(Value::from(sum)))
});

// After compile:
pipeline.wire_host_natives(&mut vm);
vm.run_raw(&bytecode);
```

### Workflow

| Step | API |
|------|-----|
| Register type + closure | `Pipeline::register_host_native(sig, closure)` |
| Typecheck user calls | Signatures forwarded to HM checker via `Compiler::register` |
| Wire before run | `Pipeline::wire_host_natives(&mut vm)` |
| Bytecode opcode | `HostInvoke` |

### Metadata-only registration

`Pipeline::register_native_function(name, namespace, sig)` registers types without a closure — for tooling or deferred wiring.

### When to use

| Approach | Use when |
|----------|----------|
| Host natives | Embedding zero-script in a Rust app; hot callbacks; sandboxed API surface |
| `extern` / `dload` | Calling existing C libraries; plugins as `.so` files |

---

## `assert` (`prelude::test`)

Auto-imported from the virtual `prelude::test` module. Returns a `Result` — it does **not** abort by itself.

### Forms

| Form | Result on success | Result on failure |
|------|-------------------|-------------------|
| `assert(cond)` | `Result::Ok(())` | `Result::Err("assertion failed")` |
| `assert(cond, msg)` | `Result::Ok(())` | `Result::Err(msg)` |

`cond` must be `bool`; `msg` must be `string`. Propagate with `?` in a result-mode function, or `match` the value:

```0s
fn must_be_pos(int n) {
    assert(n > 0, "expected positive")?;
    return n;
}
```

Rebind the short name with `use prelude::test::assert as check;` if you need `assert` free for something else.

See `examples/assert.0s`.

---

## `panic`

Keyword that aborts the program with a string message. Writes `panic: <msg>` and stops the VM; the CLI exits with code `1`. Language panics also fail `zero-script test`.

```0s
panic "unreachable";
panic format "bad index %i", i;
```

Unlike `raise`, `panic` is not recoverable with `?` / `match`. Prefer `assert` + `?` when callers should handle failure.

See `examples/panic.0s`.

---

## VM opcodes (reference)

User code does not name these directly; the compiler emits them:

| Opcode | Role |
|--------|------|
| `PRINT` | Write string to output |
| `FORMAT` | Build formatted string from specifiers |
| `FfiLoad` | `dload` |
| `DeclareFFI` | `declare` |
| `FfiInvoke` | `invoke` |
| `HostInvoke` | Host-registered closure |
| `Panic` | Abort after writing `panic: <msg>` |

---

## Related documents

| Document | Contents |
|----------|----------|
| [FFI tutorial](../tutorial/07-ffi.md) | End-to-end C interop |
| [Keywords](keywords.md) | `print`, `dload`, etc. |
| [Types](types.md) | Format specifier type rules |
| [Getting Started](../getting-started.md) | libffi prerequisites |
