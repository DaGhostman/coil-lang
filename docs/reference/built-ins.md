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
| `dload` | Expression | `dlopen` a shared library |
| `declare` | Expression | Register FFI function signature |
| `invoke` | Expression | Call registered FFI function |
| `done` | Expression | `true` if a coroutine handle is finished |
| Host natives | Embedder API | Rust closures from `Pipeline::register_host_native` |

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
| `%b` | `int` | Binary representation (VM-specific) |
| `%x` | `int` | Hex representation (VM-specific) |
| `%u` | `int` | Unsigned-style address rendering |
| `%p` | `int` | Pointer-style hex |
| `%%` | *(none)* | Literal `%` |

**Not supported:** `%d` (rejected by typechecker — use `%i`).

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

## `dload`

Load a native shared library at runtime.

### Syntax

```0s
dload(path_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `path_expr` | `string` | Path or name passed to `dlopen` |

### Returns

Library handle as `int` (heap library object address). Returns `-1` on failure.

### Example

```0s
let lib = dload("libsum.so");
```

### Notes

- Requires libffi-enabled build.
- Prefer full paths when cwd is unpredictable.
- Same mechanism as the string in `extern "..." { ... }` blocks.

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

## `declare`

Register a C function signature in a loaded library.

### Syntax

```0s
declare(lib, name, (arg_types...), ret_type)
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Handle from `dload` |
| `name` | `string` | Symbol name for `dlsym` |
| `(arg_types...)` | Tuple of FFI tags | One tag per parameter |
| `ret_type` | FFI tag | Return type (`void` allowed) |

### Returns

Function id (`int`), or `-1` if symbol missing or libffi rejects signature.

### FFI type tags

Either enum constructors or bare names:

```0s
enum FFIType { Int, Float, String, Void }

declare(lib, "f", (FFIType::Int, FFIType::String), FFIType::Int);
declare(lib, "g", (int, float), void);   // bare names
```

| Tag | Meaning |
|-----|---------|
| `int` / `FFIType::Int` | 64-bit integer |
| `float` / `FFIType::Float` | 64-bit float |
| `string` / `FFIType::String` | C string |
| `void` / `FFIType::Void` | No return value only |

`void` cannot appear as an argument type.

---

## `invoke`

Call a function registered with `declare`.

### Syntax

```0s
invoke(lib, fn_id, (args...))
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Same library handle |
| `fn_id` | `int` | Id from `declare` |
| `(args...)` | Tuple of values | Must match declared arity and types |

### Returns

Value per declared return type. `void` functions push nothing meaningful — do not rely on a return value.

### Example

```0s
print "%i", invoke(lib, sum_id, (40, 2));
```

### Typechecker note

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

## What is NOT a builtin

There is **no general standard library** yet. The following are **not** built-in — you must provide your own functions or FFI:

| Category | Examples |
|----------|----------|
| Collections API | `sort`, iterators (`push` / `len` are builtins) |
| String ops | slice, trim (concat via `+` and `format` are supported) |
| Math | `sin`, `sqrt`, `random` |
| File I/O | `read_file`, `write_file` |
| Networking | sockets, HTTP |
| Concurrency | OS threads (stackful coroutines via `async` / `yield` / `resume` / `done` are supported) |
| Memory | `alloc`, `free` |

Use **FFI** to call C libraries for these capabilities, or **host natives** when embedding the VM in Rust.

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

---

## Related documents

| Document | Contents |
|----------|----------|
| [FFI tutorial](../tutorial/07-ffi.md) | End-to-end C interop |
| [Keywords](keywords.md) | `print`, `dload`, etc. |
| [Types](types.md) | Format specifier type rules |
| [Getting Started](../getting-started.md) | libffi prerequisites |
