# Examples catalog

Every runnable (or intentionally non-runnable) program under `examples/`. Run from the **repository root** unless noted otherwise:

```bash
cargo run -- examples/<file>.0s
```

Delete `out.c0s` after editing source to force recompilation.

> **Note:** The default CLI compiles a **single file** in memory. Programs with `use` / `mod` need multi-file compilation via the pipeline API (see [Modules](#modules--namespaces) below). FFI examples need **libffi** and sometimes a built shared library.

---

## Basics

Core syntax: functions, `let`, arithmetic, control flow, and I/O.

### `examples/print_literal.0s`

**Demonstrates:** Literal string output with `print`.

```0s
fn main() {
    print "hello";
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/print_literal.0s` |
| **Output** | `hello` |

---

### `examples/format_literal.0s`

**Demonstrates:** Formatted output — `print "%i", expr` pushes the value then formats it.

```0s
fn main() {
    print "%i", 42;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/format_literal.0s` |
| **Output** | `42` |

---

### `examples/let_test.0s`

**Demonstrates:** `let` bindings, reading locals, and reassignment (`x = 20;`).

```0s
fn main() {
    let x = 5;
    print "%i", x;
    let y = 10;
    print "%i", y;
    x = 20;
    print "%i", x;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/let_test.0s` |
| **Output** | `51020` |

---

### `examples/const.0s`

**Demonstrates:** Function calls mixed with arithmetic; uses `%u` format for unsigned-style integer printing.

```0s
fn sum(int a, int b) -> int {
    return a + b;
}

fn main() {
    print "%u", 2 + 2 + sum(2 + 2);
    print "%u", 2 + 2 + 2 + 2;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/const.0s` |
| **Status** | As written, `sum(2 + 2)` passes only one argument — the typechecker reports an arity error. The second `print` line alone would print `8` once fixed. |

---

### `examples/fizbuz.0s`

**Demonstrates:** `if` conditions, modulo, and independent `print` calls (FizzBuzz-style, without newlines between numbers).

```0s
fn fizbuz(int n) {
    if (n % 3) == 0 {
        print "FIZ";
    }
    if (n % 5) == 0 {
        print "BUZ";
    }
}

fn main() {
    fizbuz(1);
    fizbuz(2);
    // ... through fizbuz(15)
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/fizbuz.0s` |
| **Output** | `FIZBUZFIZFIZBUZFIZFIZBUZ` |

---

### `examples/fib.0s`

**Demonstrates:** Recursive functions, `if`, and integer arithmetic.

```0s
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    print "%i", fib(32);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/fib.0s` |
| **Output** | `2178309` |

---

### `examples/bench.0s`

**Demonstrates:** Minimal `let` + arithmetic smoke test (not a performance benchmark).

```0s
fn main() {
    let a = 5;
    let b = 7;
    let c = a + b;
    print "%i\n", c;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/bench.0s` |
| **Output** | `12` followed by a newline |

---

### `examples/call_test.0s`

**Demonstrates:** Calling a function for side effect; expression statement discards the return value.

```0s
fn add(int a, int b) -> int {
    return a + b;
}

fn main() {
    add(3, 4);
    print "done";
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/call_test.0s` |
| **Output** | `done` |

---

### `examples/gc.0s`

**Demonstrates:** String parameter passing and `print "%s"` (also exercises heap allocation / GC paths when many strings are allocated).

```0s
fn sadge(string n) {
    print "%s", n;
}

fn main() {
    sadge("Hello");
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/gc.0s` |
| **Output** | `Hello` |

---

## Enums, match, and variants

Sum types with unit, tuple, and record-shaped payloads.

### `examples/option.0s`

**Demonstrates:** Simple enum (`Option`), constructor calls, and `match` with a binding arm.

```0s
enum Option {
    None,
    Some(int),
}

fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}

fn main() {
    print "%i", unwrap(Option::Some(42));
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/option.0s` |
| **Output** | `42` |

---

### `examples/result.0s`

**Demonstrates:** Nested enums (`Result` wrapping `Option`), multiple `match` arms sharing an outer tag with different inner patterns, and inner-pattern dispatch at runtime.

```0s
enum Result {
    Ok(Option),
    Err(string),
}

fn unwrap_result(Result r) -> int {
    return match r {
        Result::Err(_) => -1,
        Result::Ok(Option::Some(v)) => v,
        Result::Ok(Option::None) => 0,
    };
}

fn main() {
    print "%i", unwrap_result(Result::Ok(Option::Some(42)));
    print "%i", unwrap_result(Result::Ok(Option::None));
    print "%i", unwrap_result(Result::Err("oops"));
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/result.0s` |
| **Output** | `420-1` |

---

### `examples/tree.0s`

**Demonstrates:** Recursive enum (`Tree::Node` contains child `Tree` values); isorecursive typing and recursive `match`.

```0s
enum Tree {
    Leaf,
    Node(int, Tree, Tree),
}

fn sum_tree(Tree t) -> int {
    return match t {
        Tree::Leaf => 0,
        Tree::Node(v, left, right) => v + sum_tree(left) + sum_tree(right),
    };
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/tree.0s` |
| **Output** | `6` |

---

### `examples/record.0s`

**Demonstrates:** Record-shaped enum variant (`Point { x: int, y: int }`), pattern destructuring in `match`, and field access (`p.x`, `p.y`).

| | |
|---|---|
| **Run** | `cargo run -- examples/record.0s` |
| **Output** | `169512` (distance² = 169, then x = 5, y = 12) |

---

### `examples/mixed.0s`

**Demonstrates:** One enum mixing **unit**, **tuple**, and **record** variant shapes; `match` arms bind payload values per shape.

```0s
enum Shape {
    Empty,
    CircleR(int),
    Rect { width: int, height: int },
    Tri { a: int, b: int, c: int },
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/mixed.0s` |
| **Output** | `025122` (areas: 0, 25, 12, 2) |

---

### `examples/nested_records.0s`

**Demonstrates:** Nested record patterns in `match` (`Wrap::W { inner: Inner::I { v }, name } => v`).

| | |
|---|---|
| **Run** | `cargo run -- examples/nested_records.0s` |
| **Output** | `99` |

---

### `examples/chained.0s`

**Demonstrates:** Chained field access across nested record enums (`o.x.v` where `x` is itself a record type).

```0s
enum Outer {
    Outer { x: Inner, y: int },
}

fn read_x_v(Outer o) -> int {
    return o.x.v;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/chained.0s` |
| **Output** | `427` (42 and 7 concatenated in one print stream) |

---

## Collections and type aliases

Tuples, arrays, dicts, and `type` aliases.

### `examples/dict.0s`

**Demonstrates:** Anonymous structurally typed records (`{ foo: 42, bar: 100 }`) and field read via `d.foo`.

```0s
fn main() {
    let d = { foo: 42, bar: 100 };
    print "%i", d.foo;
    print "%i", d.bar;
    print "%i", d.foo;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/dict.0s` |
| **Output** | `4210042` |

---

### `examples/aliases.0s`

**Demonstrates:** `type Point = (int, int);`, tuple indexing `p[0]`, and alias substitution at typecheck time (zero runtime cost).

```0s
type Point = (int, int);

fn main() {
    let p: Point = (3, 4);
    print "%i", p[0];
    print "%i", p[1];
    print "%i", distance(p);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/aliases.0s` |
| **Output** | `347` |

---

## Modules & namespaces

Multi-file projects using `use` and `mod`. Support files live under `examples/src/`.

### `examples/modules.0s`

**Demonstrates:** `use foo::sadge;` importing a function from another file; hex formatting.

```0s
use foo::sadge;

fn main() {
    sadge();
    print "%x\n", 69;
}
```

| | |
|---|---|
| **Companion** | `examples/src/foo/sadge.0s` — defines `fn sadge()` printing `420` as hex |
| **Expected output** | `1a4` (newline) then `45` — i.e. `1a4\n45` |

**Setup:** The module resolver looks for `src/foo/sadge.0s` relative to the project root (default manifest roots). The examples layout places files at `examples/src/foo/sadge.0s`, so for a working demo you need either:

- A `zero.toml` at the repo root with `roots = ["./examples/src"]`, **and**
- Multi-file compilation (`compile_src_from_file`) — the stock `cargo run` path currently compiles one file in memory and does **not** resolve `use` across files.

See [reference/modules.md](reference/modules.md) and [reference/project-config.md](reference/project-config.md) for full module workflow. Namespace integration tests live in `compiler/tests/namespace.rs`.

---

### `examples/src/foo/sadge.0s`

**Demonstrates:** Module support file; namespace `foo::sadge`, function FQN `foo::sadge::sadge`.

| | |
|---|---|
| **Run alone** | `cargo run -- examples/src/foo/sadge.0s` (if given its own `main` — this file only defines `sadge`, not `main`) |
| **Role** | Imported by `modules.0s` |

---

### `examples/src/foo.0s`

**Demonstrates:** Alternate / legacy module layout (single `foo.0s` with a top-level `sadge`).

```0s
fn sadge() {
    print "%x\n", 420;
}
```

| | |
|---|---|
| **Note** | Used in namespace tests; not the file resolved by `use foo::sadge` (that resolves to `src/foo/sadge.0s`) |

---

## FFI (foreign function interface)

Calling C from zero-script. Requires **libffi**.

### `examples/strlen.0s`

**Demonstrates:** Compile-time `extern` block — no manual `dload`/`declare` in source. The compiler emits library load and symbol registration bytecode.

```0s
extern "libc.so.6" {
    fn strlen(string s) -> int;
}

fn main() {
    let n = strlen("hello");
    print "%i", n;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/strlen.0s` |
| **Output** | `5` |
| **Requires** | `libc.so.6` available to the dynamic linker (typical on Linux) |

---

### `examples/ffi_sum.0s`

**Demonstrates:** Userland FFI — `dload`, `declare` with tuple argument types, `invoke` with tuple values.

```0s
let lib = dload("libsum.so");
let sum_id = declare(
    lib,
    "sum",
    (FFIType::Int, FFIType::Int),
    FFIType::Int,
);
print "%i", invoke(lib, sum_id, (40, 2));
```

| | |
|---|---|
| **Run** | Build the shared library first, then run |
| **Build helper** | `cc -shared -fPIC -o libsum.so examples/sum.c` (from repo root) |
| **Output** | `42` |
| **Note** | Use an absolute path in `dload(...)` for portability across working directories |

---

### `examples/sum.c`

**Demonstrates:** C companion source for `ffi_sum.0s` (not a zero-script file).

```c
int sum(int a, int b) { return a + b; }
```

| | |
|---|---|
| **Compile** | `cc -shared -fPIC -o libsum.so examples/sum.c` |

---

## Classes (partial)

### `examples/classes.0s`

**Demonstrates:** `class` declaration, `impl` method, `new Foo()`; class features are **partial** — most field/method usage remains commented out.

```0s
class Foo {
    name: String,
}

impl Foo {
    fn sadge() -> int {
        return 42;
    }
}

fn main() {
    print "%i", (2 * 2 + 3);
    let x = new Foo();
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/classes.0s` |
| **Output** | `7` |

---

## Not implemented / broken examples

### `examples/coro.0s` — does not parse

**Intended to demonstrate:** `async fn`, `yield`, and `resume` coroutines.

```0s
async fn coro() {
    print "Suspended\n";
    yield 1;
    print "Resumed\n";
}
```

| | |
|---|---|
| **Status** | **Not implemented** — `async`, `yield`, and coroutines are not part of the language yet. The parser rejects this file. |
| **Run** | Fails at parse time |

---

## Quick reference table

| File | Category | Output (if known) |
|------|----------|-------------------|
| `print_literal.0s` | Basics | `hello` |
| `format_literal.0s` | Basics | `42` |
| `let_test.0s` | Basics | `51020` |
| `const.0s` | Basics | Type error (arity); fix `sum` call to run |
| `fizbuz.0s` | Basics | `FIZBUZFIZFIZBUZFIZFIZBUZ` |
| `fib.0s` | Basics | `2178309` |
| `bench.0s` | Basics | `12\n` |
| `call_test.0s` | Basics | `done` |
| `gc.0s` | Basics | `Hello` |
| `option.0s` | Enums | `42` |
| `result.0s` | Enums | `420-1` |
| `tree.0s` | Enums | `6` |
| `record.0s` | Enums / records | `169512` |
| `mixed.0s` | Enums | `025122` |
| `nested_records.0s` | Enums | `99` |
| `chained.0s` | Enums / fields | `427` |
| `dict.0s` | Collections | `4210042` |
| `aliases.0s` | Types | `347` |
| `modules.0s` | Modules | `1a4\n45` (needs module setup) |
| `src/foo/sadge.0s` | Modules | (support file) |
| `src/foo.0s` | Modules | (support file) |
| `strlen.0s` | FFI | `5` |
| `ffi_sum.0s` | FFI | `42` |
| `sum.c` | FFI | (C source, not `.0s`) |
| `classes.0s` | Classes | `7` |
| `coro.0s` | — | **Parse error** (unimplemented) |

## Running tests that mirror examples

The compiler crate runs many of these as golden tests:

```bash
cargo test -p compiler --test pipeline
```

This is useful to verify expected output without invoking the full CLI archive path.
