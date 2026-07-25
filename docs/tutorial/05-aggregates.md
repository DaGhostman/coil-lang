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
let t = (10, 20);
print "%i", t[0];   // OK — index 0
print "%i", t[5];   // compile error: tuple index 5 out of bounds for tuple of length 2
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
See [Operators — Aggregate arithmetic](../reference/operators.md).

For linear algebra on vectors/matrices, use named helpers (`dot`,
`cross`, `matmul`) from [Built-ins](../reference/built-ins.md#linear-algebra-dot--matmul--cross)
rather than overloading `*` / `**`.

---

## Arrays

An **array** is a homogeneous collection — every element has the same type.

### Array literals

```coil
let nums = [1, 2, 3];
let empty: [int] = [];
```

All elements in a literal must share one type. Mixing types is a compile error:

```coil
let bad = [1, "x"];   // error: array element type mismatch
```

### Array types: dynamic and fixed-length

| Syntax | Meaning |
|--------|---------|
| `[T]` | Dynamic length — size known only at runtime |
| `[T; N]` | Fixed length `N` — size is part of the type |

Examples:

```coil
fn sum_fixed([int; 3] arr) -> int {
    return arr[0] + arr[1] + arr[2];
}

fn head([int] arr) -> int {
    return arr[0];
}
```

A literal like `[1, 2, 3]` infers the fixed type `[int; 3]`. Function parameters annotated as `[int]` are dynamic — useful when data comes from external sources (SQL rows, JSON arrays) whose length is not known statically.

### Element-wise arithmetic on numeric arrays

Fixed-length `[T; N]` arrays zip element-wise when lengths match.
Dynamic `[T] ⊕ [T]` is a **hard type error** — promote to `[T; N]`
(literals already do) or broadcast a scalar:

```coil
[1, 2] + [3, 4];   // [4, 6]  (literal → [int; 2])
[1, 2] + 3;        // [4, 5]
```

### Array indexing

Indexing uses the same `arr[i]` syntax as tuples.

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

**Dynamic-length arrays** (`[T]`):

- No compile-time out-of-bounds check. Runtime access may return a sentinel value if the index is invalid.

### Growing arrays with `arr[] =` and `len`

`arr[] = value` appends in place (empty index is only legal on the left of `=`). `len(arr)` returns the current runtime length.

```coil
fn main() {
    let a = [1, 2];
    a[] = 3;
    a[] = 4;
    print "%i", len(a); // 4
    print "%i", a[3];  // 4
}
```

The appended value must match the element type. After append, a fixed literal array such as `[int; 2]` is treated as dynamic `[int]` for later indexing checks.

---

## Dicts (anonymous records)

A **dict** (anonymous record) is written with curly braces and named fields:

```coil
let d = { foo: 42, bar: 100 };
print "%i", d.foo;   // 42
print "%i", d.bar;   // 100
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
let d = { foo: 42 };
print "%i", d.bar;   // error: Cannot find field `bar` on record `{ foo: int }`
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
type Point = (int, int);

fn distance(Point p) -> int {
    let dx = p[0];
    let dy = p[1];
    return dx + dy;
}

fn main() {
    let p: Point = (3, 4);
    print "%i", p[0];          // 3
    print "%i", p[1];          // 4
    print "%i", distance(p);   // 7
}
```

Aliases are substituted at **typecheck time** only. They have **zero runtime cost** — no extra bytecode is emitted.

Scoping rules:

- Aliases are lexical: a block or function may define an alias that shadows an outer alias.
- Declaring `type X = T;` twice in the same scope is a typechecking diagnostic.

See `examples/aliases.hy` for a complete runnable example.

---

## Choosing the right aggregate

| Feature | Tuple `(a, b)` | Array `[T]` / `[T; N]` | Dict `{ k: v }` | Enum record variant |
|---------|----------------|------------------------|-----------------|---------------------|
| Element types | Heterogeneous | Homogeneous | Named fields, any types per field | Named fields, fixed by enum declaration |
| Size | Fixed at compile time | Fixed (`[T; N]`) or dynamic (`[T]`) | Fixed by literal | Fixed by variant declaration |
| Access | Index `t[i]` | Index `arr[i]` | Field `d.foo` | Field `p.x` or pattern match |
| Type identity | Structural `(int, string)` | Structural `[int; 3]` | Structural `{ foo: int }` | Nominal — tied to enum name |
| Variants | None | None | None | Multiple variants (sum type) |
| Typical use | Return multiple values | Collections, buffers | Ad-hoc structs, config maps | Domain types with tagged variants |

**Rule of thumb:**

- Use a **tuple** when you need a small, fixed bundle of different types (coordinates, `(value, error)` pairs).
- Use an **array** when all elements share one type and you need indexing.
- Use a **dict** when you want named fields without declaring an enum.
- Use an **enum record variant** when the value is part of a larger sum type with distinct cases (`Some` / `None`, `Ok` / `Err`).

---

## Runnable examples

| File | Demonstrates |
|------|--------------|
| `examples/array_grow.hy` | Growing arrays with `arr[] =` and `len` |
| `examples/dict.hy` | Dict literals and field access |
| `examples/aliases.hy` | Type aliases with tuples |
| `examples/record.hy` | Enum record variants (contrast with dicts) |

Run any example from the project root:

```bash
cargo run -- examples/dict.hy
```

---

## See also

- [Records and Fields](04-records-and-fields.md) — enum record variants vs anonymous dicts
- [Types and Variables](02-types-and-variables.md) — type annotations and inference
- [Types reference](../reference/types.md) — complete type system reference
