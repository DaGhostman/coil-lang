# Examples catalog

Every runnable (or intentionally non-runnable) program under `examples/`. Run from the **repository root** unless noted otherwise:

```bash
cargo run -- examples/<file>.0s
```

Delete `out.c0s` after editing source to force recompilation.

> **Note:** The CLI uses multi-file discovery (`Pipeline::compile_src_from_file`) when a `zero.toml` is present, so `use` / `mod` examples such as `modules.0s` work from `cargo run`. FFI examples need **libffi** and sometimes a built shared library (`libsum.so` / `libsum.dylib` / `sum.dll`).

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

### `examples/string_fmt.0s`

**Demonstrates:** String concatenation with `+` and the `format` expression returning a string.

```0s
fn main() {
    let a = "hello";
    let b = "world";
    print "%s", a + " " + b;
    let s = format "%i-%s", 42, "x";
    print "%s", s;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/string_fmt.0s` |
| **Output** | `hello world42-x` |

---

### `examples/show_tuple.0s`

**Demonstrates:** `%v` structural Show for tuples and anonymous records.

```0s
fn main() {
    print "%v", (1, 2);
    print "%v", { a: 3, b: 4 };
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/show_tuple.0s` |
| **Output** | `(1, 2){ a: 3, b: 4 }` |

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

### `examples/named_args.0s`

**Demonstrates:** Named call-site arguments (`name: value`), including a positional prefix followed by named args.

```0s
fn greet(string name, int age) {
    print "%s", name;
    print "%i", age;
}

fn main() {
    greet(name: "Ada", age: 36);
    greet("Grace", age: 40);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/named_args.0s` |
| **Output** | `Ada36Grace40` |

---

### `examples/variadic.0s`

**Demonstrates:** Trailing rest parameters (`T... name`) packing into a dynamic array, including an empty rest and named fixed args followed by positional rest.

```0s
fn sum(int... xs) -> int { /* len + loop */ }
fn greet(string name, string... extras) -> string { /* concat */ }

fn main() {
    print "%i", sum(1, 2, 3);           // 6
    print "%i", sum();                  // 0
    print "%s", greet(name: "Hi", "!", "?"); // Hi!?
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/variadic.0s` |
| **Output** | `60Hi!?` |

---

### `examples/const.0s`

**Demonstrates:** Immutable `const` bindings (reassignment is rejected by the typechecker).

```0s
fn main() {
    const answer = 42;
    print "%i", answer;
    const greeting = "hi";
    print "%s", greeting;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/const.0s` |
| **Output** | `42hi` |

---

### `examples/for_break.0s`

**Demonstrates:** C-style `for` with `continue` and `break` (sum `0+1+2+4+5+6` = `18`).

```0s
fn main() {
    let sum = 0;
    for (let i = 0; i < 10; i = i + 1) {
        if i == 3 { continue; }
        if i == 7 { break; }
        sum = sum + i;
    }
    print "%i", sum;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/for_break.0s` |
| **Output** | `18` |

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

**Demonstrates:** Recursive functions, `if`, and integer arithmetic (smoke / docs).

```0s
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    print "%i", fib(10);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/fib.0s` |
| **Output** | `55` |

For the release CPU / dispatch regression (`fib(32)` → `2178309`), use
`examples/fib_bench.0s` instead.

---

### `examples/fib_bench.0s`

**Demonstrates:** Same recursive Fibonacci as `fib.0s`, but with `fib(32)` for
release `poop` / `perf_metrics` regression measurement.

| | |
|---|---|
| **Run** | `cargo run --release -- examples/fib_bench.0s` |
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

**Demonstrates:** Built-in `Option`, constructor calls, and `match` with a binding arm.

```0s
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

**Demonstrates:** Built-in `Result` wrapping `Option`, multiple `match` arms sharing an outer tag with different inner patterns, and inner-pattern dispatch at runtime.

```0s
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

### `examples/assert.0s`

**Demonstrates:** `prelude::test::assert` returning `Result<(), string>`, with `?` propagation and matched error messages.

| | |
|---|---|
| **Run** | `cargo run -- examples/assert.0s` |
| **Output** | `ok,assertion failed,custom` |

---

### `examples/panic.0s`

**Demonstrates:** `panic "…"` abort (writes `panic: boom`, process exits 1).

| | |
|---|---|
| **Run** | `cargo run -- examples/panic.0s` |
| **Output** | `panic: boom` (stderr/stdout; non-zero exit) |

---

### `examples/raise_try.0s`

**Demonstrates:** `raise`, postfix `?`, and inferred `Result` return (implicit `Ok` wrapping).

```0s
fn parse_pos(int n, int is_neg) {
    if is_neg == 1 {
        raise "neg";
    }
    return n;
}

fn double_pos(int n, int is_neg) {
    let v = parse_pos(n, is_neg)?;
    return v * 2;
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/raise_try.0s` |
| **Output** | `10,neg` |

---

### `examples/coalesce.0s`

**Demonstrates:** `??` on `Option` and `Result` (`Err` is swallowed on Result).

| | |
|---|---|
| **Run** | `cargo run -- examples/coalesce.0s` |
| **Output** | `bar,hi,7,9` |

---

### `examples/optional_chain.0s`

**Demonstrates:** `?.` optional field access on `Option` plus `??` fallback.

| | |
|---|---|
| **Run** | `cargo run -- examples/optional_chain.0s` |
| **Output** | `42,0` |

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

### `examples/derive_show_eq.0s`

**Demonstrates:** Header `derive Show, Eq, Ord` on enums and classes — structural `%v`, `==`, and `<` without hand-written `impl`s.

```0s
enum Color derive Show, Eq, Ord {
    Red,
    Blue,
}

enum Point derive Show, Eq {
    Origin,
    Point { x: int, y: int },
}

class Cell derive Show, Eq {
    value: int,
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/derive_show_eq.0s` |
| **Output** | `Color::Red,true,false,true,Point::Point { x: 5, y: 12 },true,false,Cell { value: 42 },true,false` |

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

## IO streams

Virtual `io` module (`use io::*;`), `byte` / `[byte]` buffers, files, EOF, text helpers, UDP.

### `examples/io_bytes.0s`

**Demonstrates:** `byte` annotation, `[byte]` literal coercion, `len` / index.

| | |
|---|---|
| **Run** | `cargo run -- examples/io_bytes.0s` |
| **Output** | `25532` |

### `examples/io_file.0s`

**Demonstrates:** `open` / `write_all` / `read_to_end` / `close` round-trip; `Result` + `?`.

| | |
|---|---|
| **Run** | `cargo run -- examples/io_file.0s` |
| **Output** | `2` |

### `examples/io_eof.0s`

**Demonstrates:** Non-blocking `read` on an empty file returns `Ok(None)` (EOF).

| | |
|---|---|
| **Run** | `cargo run -- examples/io_eof.0s` |
| **Output** | `eof` |

### `examples/io_text.0s`

**Demonstrates:** `from_bytes` (`[byte]` → UTF-8 `string`) and `to_bytes` (`string` → `[byte]`).

| | |
|---|---|
| **Run** | `cargo run -- examples/io_text.0s` |
| **Output** | `hello2` |

### `examples/io_udp.0s`

**Demonstrates:** `use io::net::udp::*;` — `bind` / `local_port` / `send_to` / `recv_from_wait` localhost echo.

| | |
|---|---|
| **Run** | `cargo run -- examples/io_udp.0s` |
| **Output** | `2` |

### `examples/io_nested_host.0s`

**Demonstrates:** Nested IO HostInvoke — `read_to_end(open(...)?)` must leave the
stream (not the outer native id) on the stack for `MakeTuple`.

| | |
|---|---|
| **Run** | `cargo run -- examples/io_nested_host.0s` |
| **Output** | `3` |

### `examples/io_nested_write.0s`

**Demonstrates:** Nested IO HostInvoke with outer arity > 1 —
`write_all(open(...)?, buf)` must pack the stream (not the outer native id)
into `MakeTuple`.

| | |
|---|---|
| **Run** | `cargo run -- examples/io_nested_write.0s` |
| **Output** | `2` |

See [Tutorial 10 — IO streams](tutorial/10-io-streams.md).

---

## Collections and type aliases

Tuples, arrays, dicts, and `type` aliases.

### `examples/array_grow.0s`

**Demonstrates:** Growing arrays with `push`, reading the runtime length with `len`, and indexing appended elements.

```0s
fn main() {
    let a = [1, 2];
    push(a, 3);
    push(a, 4);
    print "%i", len(a);
    print "%i", a[0];
    print "%i", a[3];
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/array_grow.0s` |
| **Output** | `414` |

---

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

### `examples/generic_alias.0s`

**Demonstrates:** Parametric type aliases — `type Pair<T> = (T, T);` expands `Pair<int>` to `(int, int)` at typecheck time.

```0s
type Pair<T> = (T, T);

fn main() {
    let p: Pair<int> = (3, 4);
    print "%i", p[0] + p[1];
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/generic_alias.0s` |
| **Output** | `7` |

---

### `examples/generic_enum.0s`

**Demonstrates:** User generic enums — `enum Box<T> { Empty, Full(T) }` with construct/match typed as `Box<int>` (same machinery as builtin `Option` / `Result`).

```0s
enum Box<T> {
    Empty,
    Full(T),
}

fn unwrap(Box<int> b) -> int {
    return match b {
        Box::Empty => 0,
        Box::Full(v) => v,
    };
}

fn main() {
    print "%i", unwrap(Box::Full(7));
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/generic_enum.0s` |
| **Output** | `7` |

---

### `examples/generics.0s`

**Demonstrates:** Generic functions with a `Num` trait bound — one `add<T: Num>` body used at `int` and `float` call sites.

```0s
fn add<T: Num>(T a, T b) -> T {
    return a + b;
}

fn main() {
    print "%i", add(3, 4);
    print "%i", add(10, 32);
    print "%f", add(1.5, 2.5);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/generics.0s` |
| **Output** | `7424.0427` |

---

### `examples/generic_print.0s`

**Demonstrates:** Format `%v` via the `Show` trait — builtin instances for
primitives, a user `impl Show<Point>`, and `format "%v"` parity with `print`.

| | |
|---|---|
| **Run** | `cargo run -- examples/generic_print.0s` |
| **Output** | `42hi1.5true(3,4)99` |

---

### `examples/existential_show.0s`

**Demonstrates:** Bare-class existential `Show` in a parameter type. The call
`print_any(42)` packs the concrete value with its `Show<int>` dictionary, and
`show(x)` dispatches through that stored dictionary.

```0s
fn print_any(Show x) {
    print "%s", show(x);
}

fn main() {
    print_any(42);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/existential_show.0s` |
| **Output** | `42` |

---

### `examples/hkt_container.0s`

**Demonstrates:** Unary higher-kinded traits (`Container<F: * -> *>`) with
an `impl Container<Option>`, a polymorphic instance method `first<A>`, and a
generic caller `get<F: Container, A>(F<A>) -> A`.

| | |
|---|---|
| **Run** | `cargo run -- examples/hkt_container.0s` |
| **Output** | `42` |

---

### `examples/hkt_bifunctor.0s`

**Demonstrates:** Binary higher-kinded traits
(`Bifunctor<F: * -> * -> *>`) with an `impl Bifunctor<Result>` and a
generic caller whose parameter has both an explicit kind and bound:
`F: * -> * -> *, Bifunctor`.

| | |
|---|---|
| **Run** | `cargo run -- examples/hkt_bifunctor.0s` |
| **Output** | `42` |

---

### `examples/multiparam.0s`

**Demonstrates:** Multi-parameter trait `Convert<A, B>` with a `where`
clause on a generic function (`fn apply_cast<A, B>(A x) -> B where Convert<A, B>`).

| | |
|---|---|
| **Run** | `cargo run -- examples/multiparam.0s` |
| **Output** | `42` |

---

### `examples/into.0s`

**Demonstrates:** Prelude conversion trait `Into` —
`impl Into<Fahrenheit> for Celsius` and `let f: Fahrenheit = c.into();`.

| | |
|---|---|
| **Run** | `cargo run -- examples/into.0s` |
| **Output** | `32` |

---

### `examples/trait_dict.0s`

**Demonstrates:** User trait dictionaries, method sugar, and dictionary
forwarding through a nested generic call.

| | |
|---|---|
| **Run** | `cargo run -- examples/trait_dict.0s` |
| **Output** | `4242` |

---

### `examples/trait_default.0s`

**Demonstrates:** An omitted default method calling a sibling implementation
through the same dictionary.

| | |
|---|---|
| **Run** | `cargo run -- examples/trait_default.0s` |
| **Output** | `42` |

---

### `examples/superclass_ord.0s`

**Demonstrates:** Typeclass superclass / implied bounds —
`trait Ordered<T: Equal>` stores `Equal` as a superclass; `fn cmp_eq<T: Ordered>`
can call `eq_val` without writing `T: Ordered + Equal`. Flattened dict layout
is subclass methods then superclass methods.

| | |
|---|---|
| **Run** | `cargo run -- examples/superclass_ord.0s` |
| **Output** | `truetruefalse` |

---

### `examples/constraint_kind.0s`

**Demonstrates:** Constraint-kind parameters
(`fn choose<c: * -> Constraint, T: c>(...)`). The body first selects
`c = Ordered` through `lt_val`, then calls `eq_val` through Ordered's
`Equal` superclass dictionary slot.

| | |
|---|---|
| **Run** | `cargo run -- examples/constraint_kind.0s` |
| **Output** | `42` |

---

### `examples/assoc_type.0s`

**Demonstrates:** Associated types — `type Elem;` in a trait,
`type Elem = int;` in the impl, bare `Elem` as a method return type,
open `C::Elem` under `C: Collect`, and a ground `take_head(Option::Some(42))`
call that pins the projection to `int`.

| | |
|---|---|
| **Run** | `cargo run -- examples/assoc_type.0s` |
| **Output** | `42` |

---

### `examples/gat_pointer.0s`

**Demonstrates:** Generic associated types — `type Ref<T>;` in a
trait, `type Ref<T> = T;` in the impl, and an applied projection
`P::Ref<A>` pinned by the selected `Pointer<Option>` instance.

```0s
trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}

impl Pointer<Option> {
    type Ref<T> = T;
    fn deref<T>(Option<T> ptr) -> T { /* ... */ }
}

fn get<P: * -> *, Pointer, A>(P<A> ptr) -> P::Ref<A> {
    return deref(ptr);
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/gat_pointer.0s` |
| **Output** | `42` |

---

### `examples/polyfn.0s`

**Demonstrates:** First-class generic functions, multi-instantiation,
constrained apply-site dictionaries, rank-n `forall` parameters, and
captured dictionary evidence that survives returning a PolyFn
(`app_dict_arity=0` at the use site).

| | |
|---|---|
| **Run** | `cargo run -- examples/polyfn.0s` |
| **Output** | `424.0424242` |

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

**Demonstrates:** Compile-time `extern` block — no manual `dload`/`declare` in source. The compiler emits library load and symbol registration bytecode (unwraps `Result`, panics on failure).

```0s
extern "c" {
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
| **Requires** | Platform C library via `extern "c"` (`libc.so.6` / `libSystem` / `ucrtbase`, …) |

---

### `examples/ffi_sum.0s`

**Demonstrates:** Userland FFI — `dload` / `declare` / `invoke` each return `Result<_, Error>`; unwrap with `match` (or `?`). Check `e.kind` for typed recovery.

```0s
use ffi::*;
use ffi::types::*;

fn main() {
    let lib = match dload("sum") {
        Result::Ok(h) => h,
        Result::Err(e) => panic e.message,
    };
    let sum_id = match declare(lib, "sum", (Int, Int), Int) {
        Result::Ok(id) => id,
        Result::Err(e) => panic e.message,
    };
    let n = match invoke(lib, sum_id, (40, 2)) {
        Result::Ok(v) => v,
        Result::Err(e) => panic e.message,
    };
    print "%i", n;
}
```

| | |
|---|---|
| **Run** | Build the shared library first, then run |
| **Build helper** | Linux: `cc -shared -fPIC -o examples/libsum.so examples/sum.c`; macOS: `-dynamiclib` → `libsum.dylib`; Windows: `clang -shared` → `sum.dll` |
| **Output** | `42` |
| **Note** | `dload("sum")` resolves via `platform_lib_names` + `[ffi] search_paths` (no absolute path required) |

---

### `examples/sum.c`

**Demonstrates:** C companion source for `ffi_sum.0s`, `ffi_struct_ret.0s`, and `ffi_callback_ret.0s` (not a zero-script file).

```c
int sum(int a, int b) { return a + b; }
/* also: make_point, get_doubler, … */
```

| | |
|---|---|
| **Compile (Linux)** | `cc -shared -fPIC -o examples/libsum.so examples/sum.c` |
| **Compile (macOS)** | `cc -dynamiclib -o examples/libsum.dylib examples/sum.c` |
| **Compile (Windows)** | `clang -shared -o examples/sum.dll examples/sum.c` |

---

### `examples/ffi_struct_ret.0s`

**Demonstrates:** `extern struct` return from C unpacked into a record (`p.x` / `p.y`).

| | |
|---|---|
| **Run** | Build the platform `libsum` artifact, then `cargo run -- examples/ffi_struct_ret.0s` |
| **Output** | `34` |

---

### `examples/ffi_callback_ret.0s`

**Demonstrates:** Opaque function-pointer return (`Ptr` from `ffi::types`); prints `1` if non-null.

| | |
|---|---|
| **Run** | Build the platform `libsum` artifact, then `cargo run -- examples/ffi_callback_ret.0s` |
| **Output** | `1` |

---

### `examples/ffi_callback.0s` / `examples/ffi_array.0s`

**Demonstrates:** Callback trampolines and pointer/array FFI shapes (see source). Require the platform `libsum` shared library / libffi.

---

## Classes

### `examples/classes.0s`

**Demonstrates:** Positional ctor args, field read/write, and method calls (`self`).

```0s
class Point {
    x: int,
    y: int,
}

impl Point {
    fn sum() -> int {
        return self.x + self.y;
    }

    fn set_x(int n) {
        self.x = n;
    }
}

fn main() {
    print "%i", (2 * 2 + 3);
    let p = new Point(1, 3);
    print "%i", p.sum();
    p.set_x(5);
    print "%i", p.x;
    print "%i", p.sum();
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/classes.0s` |
| **Output** | `7458` |

### `examples/generic_class.0s`

**Demonstrates:** Generic class declaration (`class Cell<T>`), inherent
`impl Cell<T>`, constructor type inference (`new Cell(42)` → `Cell<int>`),
and a method that returns the type parameter.

```0s
class Cell<T> {
    value: T
}

impl Cell<T> {
    fn get() -> T {
        return self.value;
    }
}

fn main() {
    let c = new Cell(42);
    print "%i", c.get();
}
```

| | |
|---|---|
| **Run** | `cargo run -- examples/generic_class.0s` |
| **Output** | `42` |

---

## Coroutines

Stackful coroutines via `async fn`, `yield`, and `resume`. Phase 2 adds send/receive and `yield from`. See [Tutorial: Coroutines](tutorial/08-coroutines.md).

### `examples/coro.0s`

**Demonstrates:** Basic suspend/resume with prints between yields.

| | |
|---|---|
| **Run** | `rm -f out.c0s && cargo run -- examples/coro.0s` |
| **Output** | Suspended/resumed trace (see source) |

---

### `examples/operators.0s`

**Demonstrates:** Compound assignment, prefix/postfix increment, array and dict mutation, power, logical/bitwise operators.

| | |
|---|---|
| **Run** | `cargo run -- examples/operators.0s` |
| **Output** | `801125428falsetrue3` |

---

### `examples/coro_gen.0s`

**Demonstrates:** Generator-style counter (`yield 0`, `yield 1`, `yield 2`).

| | |
|---|---|
| **Run** | `cargo run -- examples/coro_gen.0s` |
| **Output** | `012` |

---

### `examples/coro_send.0s`

**Demonstrates:** Binding yield + `resume h with v` (ping-pong send).

| | |
|---|---|
| **Run** | `cargo run -- examples/coro_send.0s` |
| **Output** | `hello` |

---

### `examples/coro_yield_from.0s`

**Demonstrates:** `yield from` delegation.

| | |
|---|---|
| **Run** | `cargo run -- examples/coro_yield_from.0s` |
| **Output** | `012` |

---

### `examples/coro_interleave.0s`

**Demonstrates:** Two independent handles from the same parameterized `async fn`, resumed in arbitrary order, with `resume` used inline as a `print` argument.

| | |
|---|---|
| **Run** | `cargo run -- examples/coro_interleave.0s` |
| **Output** | `10,100,101,11,12,102` |

---

### `examples/coro_done.0s`

**Demonstrates:** `done(h)` builtin — `false` while suspended, `true` after completion.

| | |
|---|---|
| **Run** | `cargo run -- examples/coro_done.0s` |
| **Output** | `falsefalsetrue` |

---

### `examples/for_in_coro.0s`

**Demonstrates:** `for x in` over a coroutine — yields enter the body; completion/`return` does not; `break` mid-loop.

| | |
|---|---|
| **Run** | `cargo run -- examples/for_in_coro.0s` |
| **Output** | `01210` |

---

### `examples/range.0s`

**Demonstrates:** lazy `Range<T: Ord>` (`0..n`, `0..=n`, float bounds),
first-class range values, empty decreasing ranges.

| | |
|---|---|
| **Run** | `cargo run -- examples/range.0s` |
| **Output** | `01234012356` |

---

### `examples/for_in_array.0s`

**Demonstrates:** `for x in` over an array (`Item` = element type).

| | |
|---|---|
| **Run** | `cargo run -- examples/for_in_array.0s` |
| **Output** | `123` |

---

### `examples/for_in_tuple.0s`

**Demonstrates:** homogeneous tuple for-in (`Item` = element type).

| | |
|---|---|
| **Run** | `cargo run -- examples/for_in_tuple.0s` |
| **Output** | `123` |

---

### `examples/for_in_dict.0s`

**Demonstrates:** homogeneous dict for-in as `(string, V)` pairs (`p[1]` prints values).

| | |
|---|---|
| **Run** | `cargo run -- examples/for_in_dict.0s` |
| **Output** | `12` |

---

### `examples/for_in_custom.0s`

**Demonstrates:** user `impl IntoIterator` + `impl Iterator` on a class.

| | |
|---|---|
| **Run** | `cargo run -- examples/for_in_custom.0s` |
| **Output** | `012` |

---

## Showcase projects

Larger multi-file apps live under [`examples/projects/`](../examples/projects/README.md).
Each project has its own `zero.toml`, co-located `tests/`, and `NOTES.md`.

| Project | Focus | How to run |
|---------|--------|------------|
| `01-todo` | Classes, arrays, modules | `./examples/projects/01-todo/demo.sh` |
| `02-adventure` | Interactive stdin REPL + save/load | `./examples/projects/02-adventure/demo.sh` (or `--ci`) |
| `03-echo` | TCP + coroutines + protocol module | `./examples/projects/03-echo/demo.sh` |

Convenience from repo root:

```bash
./examples/projects/run-demos.sh    # all three demos (adventure uses transcript.txt)
./examples/projects/run-tests.sh    # co-located tests for all three
```

### Playing `02-adventure`

Reads all of stdin (`read_to_end`) then splits lines — on a TTY end with **Ctrl+D**,
or pipe a transcript. Modules: `world` / `commands` / `save` + entry `main`.

```bash
./examples/projects/02-adventure/demo.sh
```

Commands: `look`, `go north|south|east|west`, `take` / `take key`,
`inventory`, `save`, `load`, `help`, `quit`.

CI / non-interactive (always under `timeout`; canned input in `transcript.txt`):

```bash
./examples/projects/02-adventure/demo.sh --ci
```

### Per-project tests

```bash
./examples/projects/run-tests.sh
```

Or `cd` into a project and run `zero-script test` (harness is CWD-`./tests` only).
See [`examples/projects/README.md`](../examples/projects/README.md).

---

## Quick reference table

| File | Category | Output (if known) |
|------|----------|-------------------|
| `print_literal.0s` | Basics | `hello` |
| `format_literal.0s` | Basics | `42` |
| `string_fmt.0s` | Basics | `hello world42-x` |
| `show_tuple.0s` | Basics | `(1, 2){ a: 3, b: 4 }` |
| `let_test.0s` | Basics | `51020` |
| `named_args.0s` | Basics | `Ada36Grace40` |
| `variadic.0s` | Basics | `60Hi!?` |
| `const.0s` | Basics | `42hi` |
| `for_break.0s` | Basics | `18` |
| `fizbuz.0s` | Basics | `FIZBUZFIZFIZBUZFIZFIZBUZ` |
| `fib.0s` | Basics | `55` |
| `fib_bench.0s` | Perf | `2178309` |
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
| `io_bytes.0s` | IO | `25532` |
| `io_file.0s` | IO | `2` |
| `io_eof.0s` | IO | `eof` |
| `io_text.0s` | IO | `hello2` |
| `io_udp.0s` | IO | `2` |
| `io_nested_host.0s` | IO | `3` |
| `io_nested_write.0s` | IO | `2` |
| `array_grow.0s` | Collections | `414` |
| `dict.0s` | Collections | `4210042` |
| `aliases.0s` | Types | `347` |
| `generic_alias.0s` | Types | `7` |
| `generic_enum.0s` | Enums / types | `7` |
| `generics.0s` | Types | `7424.0427` |
| `generic_print.0s` | Types | `42hi1.5true(3,4)99` |
| `existential_show.0s` | Types | `42` |
| `hkt_container.0s` | Types | `42` |
| `hkt_bifunctor.0s` | Types | `42` |
| `multiparam.0s` | Types | `42` |
| `into.0s` | Types | `32` |
| `trait_dict.0s` | Types | `4242` |
| `trait_default.0s` | Types | `42` |
| `superclass_ord.0s` | Types | `truetruefalse` |
| `constraint_kind.0s` | Types | `42` |
| `assoc_type.0s` | Types | `42` |
| `gat_pointer.0s` | Types | `42` |
| `polyfn.0s` | Types | `424.0424242` |
| `operators.0s` | Operators | `801125428falsetrue3` |
| `modules.0s` | Modules | `1a4\n45` |
| `src/foo/sadge.0s` | Modules | (support file) |
| `src/foo.0s` | Modules | (support file) |
| `strlen.0s` | FFI | `5` |
| `ffi_sum.0s` | FFI | `42` |
| `ffi_struct_ret.0s` | FFI | `34` |
| `ffi_callback_ret.0s` | FFI | `1` |
| `sum.c` | FFI | (C source, not `.0s`) |
| `classes.0s` | Classes | `7458` |
| `generic_class.0s` | Classes | `42` |
| `coro.0s` | Coroutines | (see source) |
| `coro_gen.0s` | Coroutines | `012` |
| `coro_send.0s` | Coroutines | `hello` |
| `coro_yield_from.0s` | Coroutines | `012` |
| `coro_interleave.0s` | Coroutines | `10,100,101,11,12,102` |
| `coro_done.0s` | Coroutines | `falsefalsetrue` |
| `for_in_coro.0s` | Coroutines | `01210` |
| `for_in_array.0s` | Collections | `123` |
| `for_in_tuple.0s` | Collections | `123` |
| `for_in_dict.0s` | Collections | `12` |
| `for_in_custom.0s` | Collections / traits | `012` |
| `range.0s` | Ranges | `012340123561.02.03.0` |

## Running tests that mirror examples

The compiler crate runs many of these as golden tests:

```bash
cargo test -p compiler --test pipeline
```

This is useful to verify expected output without invoking the full CLI archive path.
