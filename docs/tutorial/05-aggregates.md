# Tutorial: Aggregates

zero-script gives you four ways to group data: **tuples**, **arrays**, **dicts** (anonymous records), and **enum record variants**. This chapter covers the first three plus **type aliases**, which make complex aggregate types easier to read.

---

## Tuples

A **tuple** is a fixed-size, heterogeneous product type. Each element can have a different type.

```0s
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

```0s
fn swap((int, string) pair) -> (string, int) {
    return (pair[1], pair[0]);
}
```

A one-tuple type looks like `(int,)`.

### Tuple indexing

Index tuples with integer literals: `t[0]`, `t[1]`, and so on.

When the index is a **compile-time constant**, the typechecker verifies it is in bounds:

```0s
let t = (10, 20);
print "%i", t[0];   // OK — index 0
print "%i", t[5];   // compile error: tuple index 5 out of bounds for tuple of length 2
```

Variable indices (for example `t[i]`) are not checked at compile time.

---

## Arrays

An **array** is a homogeneous collection — every element has the same type.

### Array literals

```0s
let nums = [1, 2, 3];
let empty: [int] = [];
```

All elements in a literal must share one type. Mixing types is a compile error:

```0s
let bad = [1, "x"];   // error: array element type mismatch
```

### Array types: dynamic and fixed-length

| Syntax | Meaning |
|--------|---------|
| `[T]` | Dynamic length — size known only at runtime |
| `[T; N]` | Fixed length `N` — size is part of the type |

Examples:

```0s
fn sum_fixed([int; 3] arr) -> int {
    return arr[0] + arr[1] + arr[2];
}

fn head([int] arr) -> int {
    return arr[0];
}
```

A literal like `[1, 2, 3]` infers the fixed type `[int; 3]`. Function parameters annotated as `[int]` are dynamic — useful when data comes from external sources (SQL rows, JSON arrays) whose length is not known statically.

### Array indexing

Indexing uses the same `arr[i]` syntax as tuples.

**Fixed-length arrays** (`[T; N]`):

- A **literal index** that is out of bounds is a **compile error**:
  ```0s
  let arr = [0, 1, 2];   // type [int; 3]
  let _ = arr[3];        // error: array index 3 out of bounds for array of length 3
  ```
- A **variable index** is allowed — the compiler cannot prove bounds at compile time:
  ```0s
  let i = 1;
  let _ = arr[i];        // OK
  ```

**Dynamic-length arrays** (`[T]`):

- No compile-time out-of-bounds check. Runtime access may return a sentinel value if the index is invalid.

---

## Dicts (anonymous records)

A **dict** (anonymous record) is written with curly braces and named fields:

```0s
let d = { foo: 42, bar: 100 };
print "%i", d.foo;   // 42
print "%i", d.bar;   // 100
```

### Structural typing

Dicts are **structurally typed**. Two literals with the same field names and compatible types are the same type, even if they were written in different places:

```0s
let a = { x: 1, y: 2 };
let b = { y: 3, x: 4 };   // field order does not matter
// a and b both have type { x: int, y: int }
```

There is no separate type name to declare — the shape `{ foo: int, bar: int }` *is* the type.

### Field access

Use dot notation: `d.foo`. The compiler resolves the field at compile time. Accessing a field that does not exist on the record's type is an error:

```0s
let d = { foo: 42 };
print "%i", d.bar;   // error: Cannot find field `bar` on record `{ foo: int }`
```

Duplicate field names in one literal are also rejected:

```0s
let bad = { foo: 1, foo: 2 };   // error: Duplicate field `foo`
```

### Dicts vs enum record variants

Enum variants can also use record-shaped payloads (`Point { x: int, y: int }`), and field access (`p.x`) works on those too. The difference is that enum records belong to a **sum type** with multiple variants and require pattern matching for full dispatch. Dicts are standalone structural values with no variant tag. See the comparison table below.

---

## Type aliases

Give a readable name to any type with `type Name = T;`:

```0s
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

Current limitations:

- Aliases are **global** within a file (no scoped aliases yet).
- Declaring `type X = T;` twice with the same name silently overwrites the earlier definition.

See `examples/aliases.0s` for a complete runnable example.

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
| `examples/dict.0s` | Dict literals and field access |
| `examples/aliases.0s` | Type aliases with tuples |
| `examples/record.0s` | Enum record variants (contrast with dicts) |

Run any example from the project root:

```bash
cargo run -- examples/dict.0s
```

---

## See also

- [Records and Fields](04-records-and-fields.md) — enum record variants vs anonymous dicts
- [Types and Variables](02-types-and-variables.md) — type annotations and inference
- [Types reference](../reference/types.md) — complete type system reference
