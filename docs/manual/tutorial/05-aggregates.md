# Tutorial: Aggregates

coil gives you four ways to group data: **tuples**, **arrays**, **dicts** (anonymous records), and **enum record variants**. This chapter covers the first three plus **type aliases**, which make complex aggregate types easier to read.

---

## Tuples

A **tuple** is a fixed-size, heterogeneous product type. Each element can have a different type.

```coil
let pair = (42, "hello");
let triple = (1, 2, 3);
```

### One-tuples require a trailing comma

Parentheses alone do **not** create a tuple. A comma inside the parens is required:

| Expression | Meaning |
|------------|---------|
| `(a, b)` | Two-element tuple |
| `(a,)` | One-element tuple |
| `(1)` | **Not** a tuple — grouped integer expression |
| `(1 + 2)` | **Not** a tuple — grouped arithmetic |
| `((1))` | **Not** a tuple — nested grouping |

This matters for arithmetic: `(1 + 2) * 3` evaluates to `9`, not a tuple multiplied by `3`.

### Tuple types

Annotate tuple types with parentheses and commas:

```coil
fn swap((int, string) pair) -> (string, int) {
    return (pair[1], pair[0]);
}
```

A one-tuple type looks like `(int,)`.

### Tuple indexing

Index tuples with integer literals: `t[0]`, `t[1]`, and so on.

When the index is a **compile-time constant**, the typechecker verifies it is in bounds:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
let t = (10, 20);
write_all(stdout(), to_bytes(format("%i", t[0])));   // OK — index 0
write_all(stdout(), to_bytes(format("%i", t[5])));   // compile error: tuple index 5 out of bounds for tuple of length 2
```

Variable indices (for example `t[i]`) are not checked at compile time.

### Element-wise arithmetic on numeric tuples

Homogeneous tuples of `int` or `float` (or a `Num`-bounded type
parameter) support element-wise `+ - * / % **` and unary `-`, plus
scalar broadcast:

```coil
(1, 1) + (1, 1);   // (2, 2)
(1, 2) + 1;        // (2, 3)
-(1, 2);           // (-1, -2)
```

Heterogeneous tuples and mismatched arities are compile errors.
See [Operators — Aggregate arithmetic](../../references/operators.md).

For linear algebra on bare vectors/arrays, use named helpers (`dot`,
`cross`, `matmul`). For matmul with `*`, wrap with `matrix(...)` to get
a `Matrix` — see [Built-ins](../../references/math.md).

---

## Arrays

An **array** is a homogeneous **fixed-length** collection — every element has
the same type, and length `N` is part of the type (`[T; N]`).

### Array literals

```coil
let nums = [1, 2, 3];          // infers [int; 3]
let empty: [int; 0] = [];
let buf: Vec<int> = Vec::new(); // empty growable vector
```

All elements in a literal must share one type. Mixing types is a compile error:

```coil
let bad = [1, "x"];   // error: array element type mismatch
```

### Array types: fixed-length only

| Syntax | Meaning |
|--------|---------|
| `[T; N]` | Fixed length `N` — size is part of the type |
| `[T]` | **Error** (`E0119`) — use `[T; N]` or `Vec<T>` |
| `Vec<T>` | Growable heap vector |

Examples:

```coil
fn sum_fixed([int; 3] arr) -> int {
    return arr[0] + arr[1] + arr[2];
}

fn head(Vec<int> arr) -> int {
    return arr[0];
}
```

A literal like `[1, 2, 3]` infers `[int; 3]`. Locals of type `[T; N]` use
**N stack slots**; escaping (call / return / heap store) boxes into a
non-growable heap array. Prefer `Vec<T>` when length is unknown statically
(IO buffers, builders, rest packs).

### Element-wise arithmetic on numeric arrays

Fixed-length `[T; N]` arrays zip element-wise when lengths match.
Broadcast a scalar when one side is a single value:

```coil
[1, 2] + [3, 4];   // [4, 6]  (literal → [int; 2])
[1, 2] + 3;        // [4, 5]
```

### Array indexing

Indexing uses the same `arr[i]` syntax as tuples (also works on `Vec`).

**Fixed-length arrays** (`[T; N]`):

- A **literal index** that is out of bounds is a **compile error**:
  ```coil
  let arr = [0, 1, 2];   // type [int; 3]
  let _ = arr[3];        // error: array index 3 out of bounds for array of length 3
  ```
- A **variable index** is allowed — the compiler cannot prove bounds at compile time:
  ```coil
  let i = 1;
  let _ = arr[i];        // OK
  ```

**`Vec<T>`:** no compile-time out-of-bounds check on variable indices.

### Growing collections with `Vec` and `len`

`arr[] = value` append is **removed** — use `Vec` methods instead.
`len(v)` returns the current length (`v.len()` also works on `Vec`).

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let a = Vec::from([1, 2]);
    a.push(3);
    a.push(4);
    write_all(stdout(), to_bytes(format("%i", len(a)))); // 4
    write_all(stdout(), to_bytes(format("%i", a[3])));  // 4
}
```

Useful `Vec` API: `Vec::new` / `with_capacity` / `from`, plus `push`, `pop`,
`insert`, `remove`, `clear`, `reserve`, `capacity`, `len`, and index get/set.
See [Arrays and Vec](../../references/arrays.md) and `examples/vec.hy`.

---

## Dicts (anonymous records)

A **dict** (anonymous record) is written with curly braces and named fields:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
let d = { foo: 42, bar: 100 };
write_all(stdout(), to_bytes(format("%i", d.foo)));   // 42
write_all(stdout(), to_bytes(format("%i", d.bar)));   // 100
```

### Structural typing

Dicts are **structurally typed**. Two literals with the same field names and compatible types are the same type, even if they were written in different places:

```coil
let a = { x: 1, y: 2 };
let b = { y: 3, x: 4 };   // field order does not matter
// a and b both have type { x: int, y: int }
```

There is no separate type name to declare — the shape `{ foo: int, bar: int }` *is* the type.

### Field access

Use dot notation: `d.foo`. The compiler resolves the field at compile time. Accessing a field that does not exist on the record's type is an error:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
let d = { foo: 42 };
write_all(stdout(), to_bytes(format("%i", d.bar)));   // error: Cannot find field `bar` on record `{ foo: int }`
```

Duplicate field names in one literal are also rejected:

```coil
let bad = { foo: 1, foo: 2 };   // error: Duplicate field `foo`
```

### Dicts vs enum record variants

Enum variants can also use record-shaped payloads (`Point { x: int, y: int }`), and field access (`p.x`) works on those too. The difference is that enum records belong to a **sum type** with multiple variants and require pattern matching for full dispatch. Dicts are standalone structural values with no variant tag. See the comparison table below.

---

## Type aliases

Give a readable name to any type with `type Name = T;`:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
type Point = (int, int);

fn distance(Point p) -> int {
    let dx = p[0];
    let dy = p[1];
    return dx + dy;
}

fn main() {
    let p: Point = (3, 4);
    write_all(stdout(), to_bytes(format("%i", p[0])));          // 3
    write_all(stdout(), to_bytes(format("%i", p[1])));          // 4
    write_all(stdout(), to_bytes(format("%i", distance(p))));   // 7
}
```

Aliases are substituted at **typecheck time** only. They have **zero runtime cost** — no extra bytecode is emitted.

Scoping rules:

- Aliases are lexical: a block or function may define an alias that shadows an outer alias.
- Declaring `type X = T;` twice in the same scope is a typechecking diagnostic.

See `examples/aliases.hy` for a complete runnable example.

---

## Choosing the right aggregate

| Feature | Tuple `(a, b)` | Array `[T; N]` / `Vec<T>` | Dict `{ k: v }` | Enum record variant |
|---------|----------------|--------------------------|-----------------|---------------------|
| Element types | Heterogeneous | Homogeneous | Named fields, any types per field | Named fields, fixed by enum declaration |
| Size | Fixed at compile time | Fixed (`[T; N]`) or runtime (`Vec`) | Fixed by literal | Fixed by variant declaration |
| Access | Index `t[i]` | Index `arr[i]` / `v[i]` | Field `d.foo` | Field `p.x` or pattern match |
| Type identity | Structural `(int, string)` | Structural `[int; 3]` / nominal `Vec<int>` | Structural `{ foo: int }` | Nominal — tied to enum name |
| Variants | None | None | None | Multiple variants (sum type) |
| Typical use | Return multiple values | Fixed buffers / growable collections | Ad-hoc structs, config maps | Domain types with tagged variants |

**Rule of thumb:**

- Use a **tuple** when you need a small, fixed bundle of different types (coordinates, `(value, error)` pairs).
- Use an **array** `[T; N]` when all elements share one type and the length is known.
- Use a **`Vec<T>`** when the collection must grow or the length is only known at runtime.
- Use a **dict** when you want named fields without declaring an enum.
- Use an **enum record variant** when the value is part of a larger sum type with distinct cases (`Some` / `None`, `Ok` / `Err`).

---

## Nesting aggregates

Aggregates compose. A common shape is an **array of typed tuples** — a table of heterogeneous rows:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
type Row = (string, int);
type Table = [Row; 2];

fn main() {
    let people: Table = [("alice", 30), ("bob", 25)];
    for row in people {
        let (name, age) = row;
        write_all(stdout(), to_bytes(format("%s:%i", name, age)));
    }
}
```

For a growable table, use `type Table = Vec<Row>;` and `Vec::from([...])`.

See `examples/nested_aggregates.hy` for a complete runnable program (aliases + `for` + let-destructure).

You can nest the other way too: tuples of arrays, arrays of dicts, dicts whose fields are tuples, and so on. Prefer a named `type` alias when the nested shape appears more than once.

---

## Runnable examples

| File | Demonstrates |
|------|--------------|
| `examples/array_grow.hy` | `Vec::from` + `push` / `len` (growable buffers) |
| `examples/vec.hy` | Fixed `[T; N]` stack locals + `Vec` methods |
| `examples/dict.hy` | Dict literals and field access |
| `examples/aliases.hy` | Type aliases with tuples |
| `examples/nested_aggregates.hy` | `[Row; N]` tables with aliases |
| `examples/record.hy` | Enum record variants (contrast with dicts) |

Run any example from the project root:

```bash
cargo run -- examples/dict.hy
```

---

## See also

- [Records and Fields](04-records-and-fields.md) — enum record variants vs anonymous dicts
- [Types and Variables](02-types-and-variables.md) — type annotations and inference
- [Types reference](../../references/types.md) — complete type system reference
