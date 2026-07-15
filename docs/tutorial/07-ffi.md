# 07 — Foreign Function Interface (FFI)

zero-script can call code outside the VM in two ways:

1. **Compile-time `extern` blocks** — declare C functions in source; the compiler emits `dload`, `declare`, and `invoke` bytecode for you.
2. **Runtime `dload` / `declare` / `invoke`** — load a shared library and call functions entirely from script, with no recompile.

Both paths use **libffi** for dynamic dispatch. Signatures are **explicit** — there is no runtime type guessing.

---

## Prerequisites

FFI examples require **libffi** linked at build time:

| Platform | Package |
|----------|---------|
| Arch Linux | `libffi` |
| Debian / Ubuntu | `libffi-dev` |
| Fedora | `libffi-devel` |

Build the workspace, then run FFI examples:

```bash
cargo build --workspace
cargo run -- examples/strlen.0s
```

---

## Path 1: Compile-time `extern` blocks

An `extern` block names a shared library and lists function signatures. Calls to those functions look like ordinary zero-script calls.

### Example: `strlen` from libc

From `examples/strlen.0s`:

```0s
extern "libc.so.6" {
    fn strlen(string s) -> int;
}

fn main() {
    let n = strlen("hello");
    print "%i", n;
}
```

**Expected output:** `5`

### Syntax

```
extern_block ::= 'extern' STRING '{' extern_fn* '}'
extern_fn    ::= 'fn' IDENT '(' arg_list ')' ('->' type_name)? ';'
```

| Part | Meaning |
|------|---------|
| `"libc.so.6"` | Path or soname passed to `dlopen` at runtime |
| `fn strlen(string s) -> int;` | Signature only — no body, trailing `;` required |
| `strlen("hello")` | Ordinary call site; compiler wires FFI behind the scenes |

### What the compiler emits

For each `extern` function the compiler roughly:

1. Calls `dload("libc.so.6")` once and stores the library handle.
2. Calls `declare(lib, "strlen", (string), int)` and stores the function id.
3. At each call site, pushes arguments and executes `FfiInvoke`.

You do not write those steps by hand when using `extern` blocks.

### Supported FFI types in `extern`

Argument and return types must be bare primitive names:

| Type | C / libffi mapping |
|------|---------------------|
| `int` | `i64` |
| `float` | `f64` |
| `string` | `const char *` (see [String ABI](#string-abi)) |
| `void` | Return type only — functions that return nothing |

Tuples, arrays, enums, and user types are **not** valid in `extern` signatures.

---

## Path 2: Runtime `dload` / `declare` / `invoke`

Use this when you want to load libraries dynamically, pick symbols at runtime, or avoid baking library paths into compile-time `extern` blocks.

### Example: calling a custom `sum` library

**C source** (`examples/sum.c`):

```c
int sum(int a, int b) {
    return a + b;
}
```

**Build the shared library:**

```bash
cc -shared -fPIC -o libsum.so examples/sum.c
```

**zero-script** (`examples/ffi_sum.0s`):

```0s
enum FFIType {
    Int,
    Float,
    String,
    Void,
}

fn main() {
    let lib = dload("libsum.so");
    let sum_id = declare(
        lib,
        "sum",
        (FFIType::Int, FFIType::Int),
        FFIType::Int,
    );
    print "%i", invoke(lib, sum_id, (40, 2));
}
```

**Expected output:** `42`

### API reference

| Builtin | Signature | Returns |
|---------|-----------|---------|
| `dload(path)` | One string argument | Library handle (`int` at bytecode level — heap object address) |
| `declare(lib, name, args_tuple, ret)` | Four arguments | Function id (`int`), or `-1` on failure |
| `invoke(lib, fn_id, args_tuple)` | Three arguments | Value matching declared return type; nothing pushed for `void` |

**Phase 26 tuple form:** argument types and call arguments are **single tuple expressions**, not flat comma lists.

```0s
// Correct
declare(lib, "sum", (FFIType::Int, FFIType::Int), FFIType::Int);
invoke(lib, id, (40, 2));

// Wrong — diagnostics at compile time
declare(lib, "sum", FFIType::Int, FFIType::Int);
invoke(lib, id, 40, 2);
```

### FFI type tags

The third argument to `declare` and the fourth (return) must be FFI type tags. Two forms are accepted:

| Form | Example |
|------|---------|
| `FFIType` enum constructor | `FFIType::Int`, `FFIType::Float`, `FFIType::String`, `FFIType::Void` |
| Bare primitive name | `int`, `float`, `string`, `void` |

Define the enum in your script (as in `ffi_sum.0s`) or use bare names:

```0s
let id = declare(lib, "sum", (int, int), int);
```

Runtime tag mapping:

| Tag | Type |
|-----|------|
| `0` | `int` |
| `1` | `float` |
| `2` | `string` |
| `3` | `void` |

`void` is valid as a **return** type only — not as an argument type.

---

## Building C shared libraries

### Minimal workflow

1. Write C functions with C linkage and stable symbol names.
2. Compile with `-shared -fPIC`.
3. Place the `.so` where `dlopen` can find it, or pass a full path to `dload`.

```bash
cc -shared -fPIC -o libsum.so sum.c
```

### Naming and loading

| Approach | Example | Notes |
|----------|---------|-------|
| Full path | `dload("libsum.so")` | Most portable when cwd is known |
| Soname | `dload("libc.so.6")` | Depends on dynamic linker search path |
| Relative path | `dload("./vendor/libfoo.so")` | Works if cwd at runtime matches |

The `extern` block string and the `dload` path use the same `dlopen` mechanism.

### C function guidelines

- Export plain C functions (`int sum(int a, int b)`), not C++ mangled names, unless you `extern "C"`.
- Match zero-script FFI types to C types the libffi layer expects (`int` → 64-bit integer in the ABI mapping).
- Keep symbols unique within the loaded library — lookup is by name via `dlsym`.

---

## String ABI

Strings cross the FFI boundary as **NUL-terminated C strings**:

| Direction | Behavior |
|-----------|----------|
| **zero-script → C** | Heap `ObjString` is passed as `const char *` pointing at UTF-8 bytes (with NUL terminator managed by the VM). |
| **C → zero-script** | Return value is read as `char *`, **copied immediately** into a new heap `ObjString`, then returned to script. The VM does not take ownership of the C pointer. |

Implications:

- C functions must not retain pointers to zero-script string buffers after the call returns unless you copy them in C.
- C functions returning `char *` should return memory valid for the duration of the copy (static buffers, heap you still own, etc.). Do not return stack pointers.
- A null C string pointer becomes an empty string value.

---

## libffi requirement

All dynamic calls go through **libffi**:

- `DeclareFFI` prepares a libffi call interface (`ffi_cif`) at declare time.
- `FfiInvoke` marshals zero-script values into libffi arguments and invokes the function pointer.

If libffi rejects a signature combination, declare returns `-1` and the VM surfaces an error. Build failures mentioning `libffi` mean the development headers are missing — install the platform package from [Prerequisites](#prerequisites).

---

## Host embedder API (advanced)

Rust embedders can register **host closures** without shared libraries:

```rust
pipeline.register_host_native(sig, |heap, args| { /* ... */ Ok(Some(value)) });
pipeline.wire_host_natives(&mut vm);
```

This produces `HostInvoke` bytecode from `Compiler::register()`. See [Built-ins reference](../reference/built-ins.md#host-embedder-api).

---

## Limitations and safety notes

### Type system

| Limitation | Detail |
|------------|--------|
| Four FFI primitives only | No structs, pointers, or callbacks in the type system |
| Explicit signatures | Wrong arity or tag → runtime failure or `-1` from `declare` |
| `invoke` return typing | Typechecker treats `invoke` as `int`; you must match what you declared |

### Safety

| Risk | Guidance |
|------|----------|
| **Memory safety** | FFI bypasses the typechecker at the C boundary. Buggy C code can corrupt the VM process. |
| **No sandbox** | `dload` runs with the host process privileges. Only load libraries you trust. |
| **Symbol collisions** | `dlsym` resolves by name; duplicate weak symbols can bind unexpectedly. |
| **Platform ABI** | libffi maps to the platform C ABI. Struct padding, calling conventions, and 32 vs 64 bit must match your C compiler — stick to primitive-only signatures. |
| **String lifetimes** | Do not let C retain script string pointers; do not return dangling `char *` from C. |

### Operational

| Limitation | Detail |
|------------|--------|
| Failed `dload` | Returns `-1`; check before `declare` |
| Failed `declare` | Returns `-1` (missing symbol, libffi error) |
| No automatic `out.c0s` invalidation for new `.so` | Rebuild C libraries separately; bytecode does not embed `.so` contents |
| Archive version | FFI opcode layout is part of `ARCHIVE_VERSION`; stale `.c0s` files are rejected after compiler upgrades |

---

## Choosing a path

| Use `extern` when… | Use `dload`/`declare`/`invoke` when… |
|--------------------|--------------------------------------|
| Library and API are fixed at compile time | You need runtime plugin loading |
| You want ordinary call syntax | You build tooling or REPL-style scripts |
| Examples: libc `strlen`, fixed vendor SDK | Examples: hot-plug extensions, user-provided `.so` |

---

## Exercises

1. Run `examples/strlen.0s` and confirm output `5`. Change the string and predict the new length.
2. Build `libsum.so` from `examples/sum.c` and run `examples/ffi_sum.0s`.
3. Add a C function `int triple(int x) { return x * 3; }`, export it from the same `.so`, and call it via `declare`/`invoke`.
4. Try an incorrect signature (e.g. declare `sum` with one `int` argument) and observe the failure mode.

---

## Next steps

- [Built-ins reference](../reference/built-ins.md) — full `print` / FFI builtin details
- [Types reference](../reference/types.md) — what can and cannot cross the FFI boundary
- [Getting Started](../getting-started.md) — build and cache (`out.c0s`) workflow
