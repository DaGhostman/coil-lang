# 07 — Foreign Function Interface (FFI)

zero-script can call code outside the VM in two ways:

1. **Compile-time `extern` blocks** — declare C functions in source; the compiler emits `dload`, `declare`, and `invoke` bytecode for you.
2. **Runtime `dload` / `declare` / `invoke`** — load a shared library and call functions entirely from script, with no recompile.

Both paths use **libffi** for dynamic dispatch. Signatures are **explicit** — there is no runtime type guessing. Runtime `dload` / `declare` / `invoke` each return `prelude::Result` (not a sentinel `-1`).

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
extern "c" {
    fn strlen(string s) -> int;
}

fn main() {
    let n = strlen("hello");
    print "%i", n;
}
```

**Expected output:** `5`

`extern "c"` is the portable libc alias — the resolver maps it to `libc.so.6` (Linux), `libSystem` (macOS), `ucrtbase` (Windows), and so on. On load/declare failure the compiler-emitted unwrap panics with a clear message.

### Syntax

```
extern_block ::= 'extern' STRING '{' extern_fn* '}'
extern_fn    ::= 'fn' IDENT '(' arg_list ')' ('->' type_name)? ';'
```

| Part | Meaning |
|------|---------|
| `"c"` | Portable libc alias (or any path / soname / basename) |
| `fn strlen(string s) -> int;` | Signature only — no body, trailing `;` required |
| `strlen("hello")` | Ordinary call site; compiler wires FFI behind the scenes |

### What the compiler emits

For each `extern` function the compiler roughly:

1. Calls `dload(...)` once and stores the library handle (`Result` unwrapped — panic on `Err`).
2. Calls `declare(lib, "strlen", (string), int)` and stores the function id (same unwrap).
3. At each call site, pushes arguments and executes `FfiInvoke` (unwraps `Result` again).

You do not write those steps by hand when using `extern` blocks.

### Supported FFI types

In runtime `declare`, import tag constructors from the virtual `ffi::types` module (`use ffi::types::*;`) or use bare lowercase / aggregate names. `extern` blocks accept bare type names without any import:

| Form | C / libffi mapping |
|------|---------------------|
| `int` / `Int` | `i64` |
| `float` / `Float` | `f64` |
| `string` / `String` | `const char *` |
| `void` / `Void` | Return-only |
| `bool`, `int8`…`uint64`, `ptr` | Sized integers, bool, raw pointer |
| `[int]` / `(int, float)` | Lowered to `Ptr` (array/tuple buffer) |
| `Callback` | C function pointer → zero-script function |
| `extern struct Point { x: int32, y: int32 };` | Pass-by-value C struct |

Qualified paths like `ffi::types::Int` also work without a glob import. Functions with no `-> ret` in `extern` blocks default to **`void`**, not `int`.

---

## Path 2: Runtime `dload` / `declare` / `invoke`

Use this when you want to load libraries dynamically, pick symbols at runtime, or avoid baking library paths into compile-time `extern` blocks.

These names are exports of the virtual `ffi` module — **import them** before use:

```0s
use ffi::*;
use ffi::types::*;
```

### Example: calling a custom `sum` library

**C source** (`examples/sum.c`):

```c
int sum(int a, int b) {
    return a + b;
}
```

**Build the shared library** (from repo root):

```bash
# Linux
cc -shared -fPIC -o examples/libsum.so examples/sum.c
# macOS
cc -dynamiclib -o examples/libsum.dylib examples/sum.c
# Windows
clang -shared -o examples/sum.dll examples/sum.c
```

**zero-script** (`examples/ffi_sum.0s`):

```0s
use ffi::*;
use ffi::types::*;

fn main() {
    let lib = match dload("sum") {
        Result::Ok(h) => h,
        Result::Err(msg) => panic msg,
    };
    let sum_id = match declare(lib, "sum", (Int, Int), Int) {
        Result::Ok(id) => id,
        Result::Err(msg) => panic msg,
    };
    let n = match invoke(lib, sum_id, (40, 2)) {
        Result::Ok(v) => v,
        Result::Err(msg) => panic msg,
    };
    print "%i", n;
}
```

`dload("sum")` resolves to `libsum.so` / `libsum.dylib` / `sum.dll` via `platform_lib_names` and `[ffi] search_paths` in `zero.toml`. Tag constructors (`Int`, `Ptr`, …) come from `ffi::types` — you do not declare them in source.

**Expected output:** `42`

### API reference

| Builtin | Signature | Returns |
|---------|-----------|---------|
| `dload(path)` | One string argument | `Result<int, string>` — `Ok` = library handle |
| `declare(lib, name, args_tuple, ret)` | Four arguments | `Result<int, string>` — `Ok` = function id; `Err` on missing symbol / libffi error |
| `invoke(lib, fn_id, args_tuple)` | Three arguments | `Result<T, string>` — `T` from the matching `declare` return tag |

Argument types and call arguments are **single tuple expressions**, not flat comma lists.

```0s
// Correct
declare(lib, "sum", (Int, Int), Int);
invoke(lib, id, (40, 2));

// Wrong — diagnostics at compile time
declare(lib, "sum", Int, Int);
invoke(lib, id, 40, 2);
```

Unwrap with `match` (as above) or `?` inside a `Result`-returning function. Failed `dload` / `declare` no longer return `-1` or `0`.

### FFI type tags

The third argument to `declare` and the fourth (return) must be FFI type tags. Accepted forms:

| Form | Example |
|------|---------|
| In-scope `ffi::types` tag | `Int`, `Ptr`, `Callback` (after `use ffi::types::*;`) |
| Qualified virtual path | `ffi::types::Int` |
| Bare primitive / aggregate name | `int`, `void`, `[int]`, `(int, float)` |

Do **not** invent a userland `enum FFIType` — tags are compiler-virtual under `ffi::types`.

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
2. Compile as a shared library for your platform (see table below).
3. Place the artifact where `[ffi] search_paths` or the dynamic linker can find it, or pass a full path to `dload`.

| Platform | Command |
|----------|---------|
| Linux | `cc -shared -fPIC -o examples/libsum.so examples/sum.c` |
| macOS | `cc -dynamiclib -o examples/libsum.dylib examples/sum.c` |
| Windows | `clang -shared -o examples/sum.dll examples/sum.c` |

### Naming and loading

| Approach | Example | Notes |
|----------|---------|-------|
| Basename | `dload("sum")` | Resolves to `libsum.so` / `libsum.dylib` / `sum.dll` via `platform_lib_names` + `[ffi] search_paths` |
| Portable libc | `extern "c"` / `dload("c")` | Maps to the platform C library |
| Full path | `dload("/abs/path/libsum.so")` | Bypasses search when the exact file is known |
| Relative path | `dload("./vendor/libfoo.so")` | Works if cwd at runtime matches |

The `extern` block string and the `dload` path use the same resolver (`base_dir`, `[ffi] search_paths`, system search).

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

If libffi rejects a signature combination, `declare` returns `Result::Err`. Build failures mentioning `libffi` mean the development headers are missing — install the platform package from [Prerequisites](#prerequisites).

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
| Explicit signatures | Wrong arity or tag → `Result::Err` from `declare` / `invoke` |
| Struct returns | `extern struct` + `declare(..., Point)` returns a record; fields via `.x` |
| Callback returns | Opaque `Ptr` / function address — no auto-trampoline; re-`declare` to call |
| `invoke` return typing | `Result<T, string>` where `T` is refined from `let id = declare(..., ret)` when `fn_id` is that binding; else falls back |

### Safety

| Risk | Guidance |
|------|----------|
| **Memory safety** | FFI bypasses the typechecker at the C boundary. Buggy C code can corrupt the VM process. |
| **No sandbox** | `dload` runs with the host process privileges. Only load libraries you trust. |
| **Symbol collisions** | `dlsym` resolves by name; duplicate weak symbols can bind unexpectedly. |
| **Platform ABI** | libffi maps to the platform C ABI. Struct padding and calling conventions must match your C compiler. Prefer `int32`/`int64` field widths that match the C layout. |
| **String lifetimes** | Do not let C retain script string pointers; do not return dangling `char *` from C. |

### Operational

| Limitation | Detail |
|------------|--------|
| Failed `dload` | `Result::Err(string)` — match or `?`; never `-1` |
| Failed `declare` | `Result::Err` (missing symbol, libffi error) |
| `extern` failure | Compiler unwraps Results and panics with a clear message |
| No automatic `out.c0s` invalidation for new `.so` | Rebuild C libraries separately; bytecode does not embed shared-library contents |
| Archive version | FFI opcode / tag layout is part of `ARCHIVE_VERSION` (currently **21**); stale `.c0s` files are rejected after compiler upgrades |

---

## Choosing a path

| Use `extern` when… | Use `dload`/`declare`/`invoke` when… |
|--------------------|--------------------------------------|
| Library and API are fixed at compile time | You need runtime plugin loading |
| You want ordinary call syntax | You build tooling or REPL-style scripts |
| Examples: libc `strlen` via `extern "c"`, fixed vendor SDK | Examples: hot-plug extensions, user-provided shared libs |

---

## Exercises

1. Run `examples/strlen.0s` and confirm output `5`. Change the string and predict the new length.
2. Build the platform `libsum` artifact from `examples/sum.c` and run `examples/ffi_sum.0s`.
3. Add a C function `int triple(int x) { return x * 3; }`, export it from the same library, and call it via `declare`/`invoke` (unwrap the `Result`s).
4. Try an incorrect signature (e.g. declare `sum` with one `int` argument) and observe `Result::Err`.

---

## Next steps

- [Built-ins reference](../reference/built-ins.md) — full `print` / FFI builtin details
- [Types reference](../reference/types.md) — what can and cannot cross the FFI boundary
- [Getting Started](../getting-started.md) — build and cache (`out.c0s`) workflow
