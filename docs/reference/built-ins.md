# Built-ins reference

Built-in facilities provided by the language runtime and compiler — not ordinary user-defined functions in a standard library.

coil does **not** yet ship a general-purpose stdlib (no `map`, `filter`, file I/O modules, etc.). What exists today is I/O, FFI, and host-embedder hooks.

---

## Overview

| Builtin | Kind | Purpose |
|---------|------|---------|
| `print` | Statement | Write to stdout |
| `format` | Expression | Build a formatted string |
| `len` | Expression | Return an array length |
| `ffi::{dload,declare,invoke,Error,ErrorKind}` | Virtual module | Runtime FFI + typed errors (requires `use ffi::*`) |
| `ffi::types::{Int,…}` | Virtual module | FFI type-tag constructors (requires `use ffi::types::*`) |
| `io::{open,read,write,…}` | Virtual module | Non-blocking streams + sync adapters (requires `use io::*`) |
| `io::fs::{exists,realpath,…}` | Virtual module | Path/metadata (requires `use io::fs::*`) |
| `time::{timestamp,format,…}` | Virtual module | UTC timestamps, periods, sleep (requires `use time::*`) |
| `env::{args,var,exec,…}` | Virtual module | Process environment (requires `use env::*`) |
| `crypto::{sha256,hmac_sha256,…}` | Virtual module | RustCrypto host primitives (`use crypto::*`) |
| `regex::{compile,is_match,…}` | Virtual module | PCRE2 host regex (`use regex::*`; needs libpcre2) |
| `ord` / `char` | Prelude builtins | Single-byte string ↔ `byte` |
| `done` | Expression | `true` if a coroutine handle is finished |
| `prelude::{Option,Result}` | Virtual module | Auto-imported sum types |
| `prelude::ops::{Add,Eq,Into,…}` | Virtual module | Auto-imported operator / conversion traits |
| `prelude::test::assert` | Virtual module | Auto-imported; `assert(cond[, msg]) → Result<(), string>` |
| `prelude::math::{dot,matmul,cross}` | Virtual module | Auto-imported linear-algebra helpers on vectors/matrices |
| `panic` | Keyword | Abort with a string message (exit code 1) |
| Host natives | Embedder API | Rust closures from `Pipeline::register_host_native` |

Compiler builtins live in **virtual modules** (not `.hy` files). Every file gets an implicit `use prelude::*; use prelude::ops::*; use prelude::test::*; use prelude::math::*;`. FFI and **`io`** are **not** auto-imported — write `use ffi::*;` / `use io::*;` before using those APIs.

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

```coil
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

## Array append (`arr[] =`) and `len`

Append with empty index assignment; query length with `len`.

```coil
arr[] = value   // append (assignment target only)
len(arr)
```

| Form | Argument types | Returns | Behavior |
|------|----------------|---------|----------|
| `arr[] = v` | `[T]`, `T` | `[T]` (discarded in statement form) | Appends in place; promotes fixed `[T; N]` bindings to dynamic `[T]` |
| `len` | `[T]` | `int` | Current runtime length |

Empty `arr[]` is only valid as an assignment target — using it as an rvalue is a compile error.

```coil
let a = [1, 2];
a[] = 3;
print "%i", len(a); // 3
print "%i", a[2];  // 3
```

---

## Linear algebra (`dot` / `matmul` / `cross` / `Matrix`)

Auto-imported from virtual `prelude::math` (implicit `use prelude::math::*;`).

**Named helpers** do **not** overload `*` / `**` on bare tuples or arrays
(those stay element-wise; see [Operators](operators.md)).

| Helper | Arguments | Result |
|--------|-----------|--------|
| `dot(a, b)` | Equal-length homogeneous numeric vectors (tuple↔tuple or `[T; N]`↔`[T; N]`) | scalar `T` |
| `cross(a, b)` | Length-3 vectors (same container kind) | length-3 vector |
| `matmul(A, B)` | Nested fixed-length matrices: `[[T; K]; M]` × `[[T; N]; K]` | `[[T; N]; M]` (row-major) |
| `matrix(rows)` | Nested fixed-length matrix data | `Matrix<Data>` |

### `Matrix` and `*`

`matrix(...)` wraps nested static rows as a nominal `Matrix<Data>` type
(runtime is still the nested data — zero-cost). On `Matrix`:

| Op | Meaning |
|----|---------|
| `*` | **Matmul** (via `Mul`, not element-wise) |
| `+` / `-` | Element-wise zip |
| `/`, `%`, `**` | **Rejected** — `Matrix` is not `Num` |

```coil
dot((1, 2, 3), (4, 5, 6));           // 32
cross((1, 0, 0), (0, 1, 0));         // (0, 0, 1)
matmul([[1, 2], [3, 4]], [[5, 6], [7, 8]]);  // [[19, 22], [43, 50]]

let a = matrix([[1, 2], [3, 4]]);
let b = matrix([[5, 6], [7, 8]]);
let c = a * b;   // matmul → Matrix
let d = a + a;   // element-wise
```

See `examples/vec_dot.hy`, `examples/vec_matmul.hy`, and `examples/matrix_mul.hy`.

---

## `format`

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

## `dload` / `declare` / `invoke` (`ffi`)

Runtime FFI callables are exports of the virtual `ffi` module. They are **not** keywords and are **not** in scope until you import them:

```coil
use ffi::*;
use ffi::types::*;
```

Or import individually: `use ffi::dload;`, `use ffi::declare;`, `use ffi::invoke;`.

### `dload`

Load a native shared library at runtime.

```coil
dload(path_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `path_expr` | `string` | Basename, path, or alias passed to the library resolver |

Returns `Result<int, Error>` — `Ok` is the library handle (heap object address). Failure is `Err(Error)`, never `-1`.

```coil
use ffi::*;
let lib = match dload("sum") {
    Result::Ok(h) => h,
    Result::Err(e) => panic e.message,
};
```

Notes:

- Requires libffi-enabled build.
- `dload("sum")` resolves to `libsum.so` / `libsum.dylib` / `sum.dll` via `platform_lib_names` and `[ffi] search_paths`.
- `dload("c")` / `extern "c"` is the portable libc alias.
- Same resolver as the string in `extern "..." { ... }` blocks (`extern` does **not** require `use ffi::*`; it unwraps Results and panics on `e.message`).
- Check `e.kind` (`ErrorKind::LibraryNotFound`, …) for recovery; use `e.message` for display.

---

## `done`

Test whether a coroutine handle has finished.

### Syntax

```coil
done(handle_expr)
```

| Argument | Type | Description |
|----------|------|-------------|
| `handle_expr` | `coroutine<Y, S>` | Handle from calling an `async fn` |

### Returns

`bool` — `true` after the coroutine body has returned (or fallen off the end); `false` while still suspended at a `yield` or before the first `resume`.

### Example

```coil
let h = counter();
print "%z", done(h); // false
resume h;
resume h;            // completes
print "%z", done(h); // true
```

---

### `declare`

Register a C function signature in a loaded library.

```coil
declare(lib, name, (arg_types...), ret_type)
declare(lib, name, (arg_types...), ret_type, variadic)
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Handle from a successful `dload` (`Result::Ok`) |
| `name` | `string` | Symbol name for `dlsym` |
| `(arg_types...)` | Tuple of FFI tags | Fixed-prefix tags (before C `...` when variadic) |
| `ret_type` | FFI tag | Return type (`void` allowed) |
| `variadic` | `bool` (optional) | `true` for C-style varargs (`printf`-style) |

Returns `Result<int, Error>` — `Ok` is the function id; `Err` if the symbol is missing or libffi rejects the signature (`ErrorKind::SymbolNotFound`, `Libffi`, …). When `variadic` is `true`, later `invoke` calls may pass more arguments than the fixed prefix; the CIF is rebuilt per call with default C promotions on the tail.

### FFI type tags (`ffi::types`)

Tag constructors live in the virtual `ffi::types` module. After `use ffi::types::*;`, write bare `Int`, `Ptr`, `Callback`, …:

```coil
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

```coil
invoke(lib, fn_id, (args...))
```

| Argument | Type | Description |
|----------|------|-------------|
| `lib` | `int` | Same library handle |
| `fn_id` | `int` | Id from a successful `declare` |
| `(args...)` | Tuple of values | Must match declared arity (or `>=` fixed prefix when `declare` was variadic) |

Returns `Result<T, Error>` where `T` is the type recorded from the matching `declare(..., ret)` (`unit` for `void`). Bind `let id = declare(...)?` (or match) so the side table can refine later `invoke` calls.

```coil
let n = match invoke(lib, sum_id, (40, 2)) {
    Result::Ok(v) => v,
    Result::Err(e) => panic e.message,
};
print "%i", n;
```

### `Error` / `ErrorKind`

Virtual `ffi` exports (via `use ffi::*`):

| Name | Shape |
|------|-------|
| `ErrorKind` | Unit enum — `LibraryNotFound`, `SymbolNotFound`, `ArityMismatch`, `Libffi`, `InvalidSignature`, `InvalidHandle`, `Unsupported`, `Other` |
| `Error` | `Error { kind: ErrorKind, message: string }` — access `e.kind` / `e.message` |

Match on `e.kind` for recovery; use `e.message` for logging / `panic`.

---

## Compile-time FFI (`extern` blocks)

Not separate builtins — the compiler lowers extern declarations to `dload` / `declare` / `invoke` sequences. User code calls look like normal functions:

```coil
extern "c" {
    fn strlen(string s) -> int;
    fn printf(string fmt, ...) -> int;   // C varargs — bare `...`
}

fn main() {
    print "%i", strlen("hello");
}
```

`extern "c"` is the portable libc alias. Compiler-emitted setup unwraps `dload`/`declare`/`invoke` Results and panics with a clear message on failure. See [FFI tutorial](../tutorial/07-ffi.md).

---

## `io` virtual module

Non-blocking file / stdio / TCP / UDP streams. **Not** auto-imported:

```coil
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
| `io::net::tcp::{connect,listen,accept,accept_wait}` | TCP | Nested module — `use io::net::tcp::*;` |
| `io::net::udp::{bind,connect,send_to,recv_from,recv_from_wait,local_port}` | UDP | Nested module; `recv_from` → `(nbytes, host, port)` |

Buffers are **`[byte]`**. Use `from_bytes` / `to_bytes` for text. `print` still uses the `PRINT` opcode (not `stdout`). No HTTP in the VM — userland only later.

See [Tutorial 10 — IO streams](../tutorial/10-io-streams.md) and `examples/io_*.hy`.

---

## Iterator / IntoIterator

Prelude traits (virtual module — not `.hy` sources) power `for x in expr`:

```coil
trait Iterator<I> {
    type Item;
    fn next(I it) -> Option<Item>;
}

trait IntoIterator<T> {
    type Item;
    type IntoIter;
    fn into_iter(T t) -> IntoIter;
}
```

`for x in e` resolves `IntoIterator<Te>` then `Iterator<IntoIter>` with matching
`Item`, and binds `x : Item` in the body. Builtin synthesis (no ground `impl`
required) covers:

| Source | `Item` | Notes |
|--------|--------|-------|
| `[T]` / `[T; N]` | `T` | Index loop (`len` / `Index`) |
| Homogeneous `(A, …, A)` | `A` | Materialised to a temp array; hetero → diagnostic |
| Homogeneous `{ k: V, … }` | `(string, V)` | `DictEntries` then array path; hetero values → diagnostic |
| `coroutine<Y, S>` | `Y` | Resume/Done; completion value excluded from the body |

Users write ordinary `impl IntoIterator` / `impl Iterator` for custom types
(see `examples/for_in_custom.hy`). Methods are callable as UFCS
(`into_iter(x)`, `next(it)`).

---

## What is NOT a builtin

There is **no general standard library** yet. The following are **not** built-in — you must provide your own functions or FFI:

| Category | Examples |
|----------|----------|
| Collections API | `sort`; range→array materialize (lazy `a..b` / `a..=b` as `Range<T: Ord>` is supported — see [Syntax — ranges](syntax.md#ranges-lazy); `arr[] =` append / `len` / `for-in` are builtins) |
| String ops | slice, trim (concat via `+` / `format`; UTF-8 via `io::from_bytes` / `to_bytes`) |
| Math | `sin`, `sqrt`, `random` |
| High-level file helpers | path utilities beyond `io::open` / `read_to_end` / `write_all` |
| HTTP / TLS | Not in the VM (use userland on top of `io` TCP later) |
| Concurrency | — (use virtual **`thread`** module for OS threads; coroutines via `async` / `yield` / `resume` / `done` — see [Tutorial 11](../tutorial/11-threads.md) and [Tutorial 08](../tutorial/08-coroutines.md)) |
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
| Host natives | Embedding coil in a Rust app; hot callbacks; sandboxed API surface |
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

```coil
fn must_be_pos(int n) {
    assert(n > 0, "expected positive")?;
    return n;
}
```

Rebind the short name with `use prelude::test::assert as check;` if you need `assert` free for something else.

See `examples/assert.hy`.

---

## `test("…") { … }` (harness cases)

Top-level declaration used by `coil test`. The name must be a **string literal**. The body is typechecked in Result mode (`Result<(), string>`), so `assert(...)?` and `raise` work as in a result-mode function.

```coil
test("addition works") {
    assert(1 + 1 == 2)?;
}
```

Do **not** also define `fn main` in a file that uses `test(...)` cases — the compiler injects a virtual `main` for standalone runs. The `coil test` CLI runs each case in an isolated VM (so a `panic` in one case does not skip later cases) and prints `> Test "<description>" failed` on failure. Pass `--fail-fast` to stop after the first failed case.

### `#[test]` on functions

The same harness semantics apply when tests are declared as attributed functions:

```coil
#[test("addition works")]
fn add_works() {
    assert(1 + 1 == 2)?;
}

#[test]
fn multiply_works() {
    assert(3 * 4 == 12)?;
}
```

The optional string argument is the case description; when omitted, the function name is used. `#[test]` functions and `test("…") { … }` blocks may coexist in one file.

**Production compiles** (`compile`, default `cargo run`) strip harness declarations unless you pass `--include-tests`. The `coil test` command always compiles them.

---

## `panic`

Keyword that aborts the program with a string message. Writes `panic: <msg>` and stops the VM; the CLI exits with code `1`. Under `coil test`, a language panic fails the current case only (the next case still runs unless `--fail-fast` is set).

```coil
panic "unreachable";
panic format "bad index %i", i;
```

Unlike `raise`, `panic` is not recoverable with `?` / `match`. Prefer `assert` + `?` when callers should handle failure.

See `examples/panic.hy`.

---

## Primitive casts (`expr as T`)

Narrowing conversions between `int`, `float`, `byte`, and `bool` (wrapping/truncation, not checked). Semantics match Rust:

- `float as int` truncates toward zero (not `round`/`floor`). `NaN` / `±inf` follow Rust `f64 as i64` (e.g. `NaN` → `0`).
- `int as byte` keeps the low 8 bits (`257 as byte` → `1`; negatives wrap the same way, e.g. `-1 as byte` → `255`).

Examples: `n as byte`, `f as int`, `flag as bool`. The same matrix is available via `Into` (`n.into()` when the target type is known). See `examples/casts.hy`.

---

## `time` module

`use time::*;` — UTC wall clock (`timestamp`, `epoch`), `Period` arithmetic, `format` / `parse` (strftime-style), monotonic `instant_now` / `elapsed_*`, and `sleep_ms`. Errors use `TimeError` inside `prelude::Result`. File bytes are not handled here; use `io` streams.

---

## `io::fs` module

`use io::fs::*;` — `exists`, `metadata`, `list_dir`, `realpath` (canonical path when it exists), mkdir/remove/rename/copy, symlinks. Returns `prelude::Result` with `IoError`. No whole-file `read`/`write` helpers; open a `Stream` via `io::open` and use `read_to_end` / `write_all`.

---

## `env` module

`use env::*;` — `args()`, `var` / `set_var` / `remove_var`, `cwd` / `set_cwd`, `exit(code)`. `exec(program, args)` spawns a program with an argv vector (no shell). The child inherits the VM process **cwd** and **environment**; there are no per-call overrides yet. The compiler emits a **warning** when `exec` or `exit` is used. **Only `exec` is runtime-gated:** by default it returns `EnvError::ExecDisabled` unless `coil.toml` `[env] allow_exec = true`. `exit` is compile-warned only (not blocked at runtime).

---

## `crypto` module

`use crypto::*;` — one-shot and streaming hashes (`sha256`, `init` / `update` / `finalize`), HMAC, `random_bytes`, ChaCha20-Poly1305 and AES-256-GCM, Ed25519 / X25519, Argon2id, constant-time `ct_eq`. Pure Rust (RustCrypto); no OpenSSL. Argon2id uses fixed MVP params (19 MiB memory, 2 iterations, parallelism 1); salts shorter than 16 bytes are zero-padded to 16 — not OWASP-tunable.

---

## `regex` module

`use regex::*;` — PCRE2 patterns via HostInvoke (system **libpcre2** / `pcre2-sys`). Opaque `Regex` handle from `compile(pattern, flags)`.

| Surface | Types |
|---------|--------|
| `compile` | `(string, string) -> Result<Regex, RegexError>` |
| `is_match` | `(Regex, string) -> Result<bool, RegexError>` |
| `find` | `(Regex, string) -> Result<(int, int), RegexError>` — first match byte span; no match → `NoMatch` |
| `find_all` | `(Regex, string) -> Result<[(int, int)], RegexError>` — all non-overlapping spans (empty if none) |
| `captures` | `(Regex, string) -> Result<[string], RegexError>` — `[0]` full match; empty string for non-participating groups |
| `captures_all` | `(Regex, string) -> Result<[[string]], RegexError>` |
| `split` | `(Regex, string) -> Result<[string], RegexError>` |
| `replace` / `replace_all` | `(Regex, string, string) -> Result<string, RegexError>` — `$n` / `${name}` / `$$` |

**Flags** (second `compile` arg; case-sensitive; unknown letter → `Compile`): `i` caseless, `m` multiline, `s` dotall, `x` extended, `u` Unicode properties (`ucp`). UTF-8 matching is always on for coil strings. Other PCRE letters (`A`/`D`/`U`/`J`/…) are not exposed — use in-pattern verbs where PCRE2 allows.

`RegexError` variants: `Compile`, `Runtime`, `NoMatch`, `Utf8`.

---

## `ord` and `char`

Auto-imported: `ord(string) -> Result<byte, string>` (exactly one character with codepoint ≤ 255) and `char(byte) -> string`. Out-of-range `char` inputs (not in `0..=255`) return an empty string `""` rather than a `Result` error — prefer keeping the argument typed as `byte`. String literals of one such character coerce to `byte` in annotations (e.g. `let c: byte = "A";`).

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
