# Enums and Pattern Matching

Enums let you define a type as a choice among named variants. Each variant can carry no data (unit), positional values (tuple), or named fields (record). You create values with constructor syntax and branch on them with `match` expressions.

This chapter builds on [Types and Variables](02-types-and-variables.md). Record-shaped variants and field access are covered in depth in [Records and Fields](04-records-and-fields.md).

---

## Declaring an enum

An enum groups related variants under one type name. For the common “maybe” / “success or failure” shapes, prefer the **compiler-built-in** [`Option` and `Result`](09-error-handling.md) — do not redeclare those names.

```coil
enum Tree {
    Leaf,
    Node(int, Tree, Tree),
}
```

Each line inside the braces is a **variant**. Variants fall into three shapes:

| Shape | Syntax | Example |
|-------|--------|---------|
| **Unit** | name only | `None` |
| **Tuple** | name followed by types in parentheses | `Some(int)` |
| **Record** | name followed by named fields in braces | `Point { x: int, y: int }` |

A single enum can mix all three shapes. See [Mixed-shape enums](#mixed-shape-enums) below.

---

## Constructing values

Use `Enum::Variant` to build a value:

```coil
Tree::Leaf                // unit variant
Option::Some(42)          // built-in Option — tuple payload
```

### Empty parentheses mean unit

`Variant` and `Variant()` are equivalent for unit variants:

```coil
Tree::Leaf
Tree::Leaf()   // same thing
```

### Record-shaped constructors

Record variants use named fields:

```coil
Point::Point { x: 5, y: 12 }
```

Field order at the call site does not have to match the declaration — `Point::Point { y: 12, x: 5 }` is valid. See [Records and Fields](04-records-and-fields.md) for details.

---

## `match` expressions

A `match` tests a scrutinee value against a list of patterns and runs the body of the first matching arm:

```coil
match scrutinee {
    pattern1 => body1,
    pattern2 => body2,
}
```

### Pattern forms

| Pattern | Meaning | Example |
|---------|---------|---------|
| **Wildcard** | matches anything, discards the value | `_` or `default` |
| **Binding** | matches anything, binds the value to a name | `v` |
| **Constructor** | matches a specific variant and binds its payload | `Option::Some(v)` |

Constructor patterns mirror constructor syntax. A unit variant matches by name:

```coil
Option::None => 0
```

A tuple variant binds positional payloads:

```coil
Option::Some(v) => v
```

A record variant binds named fields (with shorthand — see chapter 04):

```coil
Point::Point { x, y } => x * x + y * y
```

### `match` is an expression

Every arm must produce a value, and all arm bodies must have the **same type**. The `match` expression itself evaluates to that unified type:

```coil
fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}
```

Arm bodies may also be brace blocks. A brace body accepts statements such as
`let`, `return`, `if`, and loops, followed by an optional bare expression whose
value becomes the arm's value:

```coil
Mode::Other(n) => {
    let adjusted = self.get() + n;
    adjusted
}
```

Those braces are a **block**, not a dict — `{ x: 1 }` remains a record literal.

If one arm returns `int` and another returns `string`, the compiler reports a type mismatch on the arm bodies.

Because `match` is an expression, it can appear anywhere a value is expected — in `return`, as a function argument, or on the right-hand side of a `let` binding.

---

## Worked example: built-in `Option`

From `examples/option.hy` (`Option` is a compiler builtin — no local `enum` declaration):

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
fn unwrap(Option o) -> int {
    return match o {
        Option::None => 0,
        Option::Some(v) => v,
    };
}

fn main() {
    write_all(stdout(), to_bytes(format("%i", unwrap(Option::Some(42)))));
}
```

**Expected output:** `42`

The `match` covers both variants. The `Some(v)` arm binds the inner `int` to `v` and returns it directly.

For `raise` / `?` / `??` / `?.`, see [Error handling](09-error-handling.md).

---

## Exhaustiveness checking

The compiler requires every `match` on an enum to cover **all variants**. If you omit one, you get a compile-time error:

```
Non-exhaustive match: variants not covered: `Some`
```

Cover every variant, use a wildcard arm to catch the rest, or combine both:

```coil
match o {
    Option::None => 0,
    Option::Some(v) => v,
}

// or, with a wildcard for the second variant:
match o {
    Option::None => 0,
    _ => 1,
}
```

### Unreachable arms

Two arms that match the same variant (same outer tag and, when applicable, same inner tag) make the later arm unreachable:

```
Unreachable arm: this pattern is matched by an earlier arm
```

This catches copy-paste mistakes and redundant patterns before they silently dead-code an arm.

---

## Nested constructor patterns

When a variant's payload is itself an enum, you can nest constructor patterns in a single arm:

```coil
Result::Ok(Option::Some(v)) => v
```

This matches a `Result::Ok` whose inner `Option` is `Some`, binding `v` to the inner integer.

### Inner-pattern dispatch

When **multiple arms share the same outer variant** but differ on the inner pattern, the runtime dispatches on the inner tag at match time. From `examples/result.hy`:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
fn unwrap_result(Result r) -> int {
    return match r {
        Result::Err(_) => -1,
        Result::Ok(Option::Some(v)) => v,
        Result::Ok(Option::None) => 0,
    };
}

fn main() {
    write_all(stdout(), to_bytes(format("%i", unwrap_result(Result::Ok(Option::Some(42))))));
    write_all(stdout(), to_bytes(format("%i", unwrap_result(Result::Ok(Option::None)))));
    write_all(stdout(), to_bytes(format("%i", unwrap_result(Result::Err("oops")))));
}
```

(`Option` and `Result` are compiler builtins — no local `enum` declarations.)

**Expected output:** `420-1`

| Input | Matching arm | Result |
|-------|-------------|--------|
| `Result::Ok(Option::Some(42))` | `Result::Ok(Option::Some(v))` | `42` |
| `Result::Ok(Option::None)` | `Result::Ok(Option::None)` | `0` |
| `Result::Err("oops")` | `Result::Err(_)` | `-1` |

The two `Result::Ok` arms share the outer tag but differ on the inner `Option` tag. The compiler emits a test chain so the correct arm runs based on the runtime inner value.

---

## Recursive enums

Variants can reference their own enum type, enabling tree-like structures. From `examples/tree.hy`:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
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

fn main() {
    write_all(stdout(), to_bytes(format("%i", sum_tree(Tree::Node(1,
                Tree::Node(2, Tree::Leaf(), Tree::Leaf()),
                Tree::Node(3, Tree::Leaf(), Tree::Leaf()))))));
}
```

**Expected output:** `6`

The tree has value `1` at the root, `2` on the left subtree, and `3` on the right. `0 + 2 + 0 + 3 + 0 = 6`. The recursive calls in the `Node` arm walk the structure depth-first.

---

## Mixed-shape enums

A single enum can combine unit, tuple, and record variants. From `examples/mixed.hy`:

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
enum Shape {
    Empty,
    CircleR(int),
    Rect { width: int, height: int },
    Tri { a: int, b: int, c: int },
}

fn area(Shape s) -> int {
    return match s {
        Shape::Empty => 0,
        Shape::CircleR(r) => r * r,
        Shape::Rect { width, height } => width * height,
        Shape::Tri { a, b, c } => (a + b + c) / 3,
    };
}

fn main() {
    write_all(stdout(), to_bytes(format("%i", area(Shape::Empty))));
    write_all(stdout(), to_bytes(format("%i", area(Shape::CircleR(5)))));
    write_all(stdout(), to_bytes(format("%i", area(Shape::Rect { width: 3, height: 4 }))));
    write_all(stdout(), to_bytes(format("%i", area(Shape::Tri { a: 1, b: 2, c: 3 }))));
}
```

**Expected output:** `025122`

| Variant | Shape | Computation | Result |
|---------|-------|-------------|--------|
| `Empty` | unit | constant | `0` |
| `CircleR(5)` | tuple | `5 * 5` | `25` |
| `Rect { width: 3, height: 4 }` | record | `3 * 4` | `12` |
| `Tri { a: 1, b: 2, c: 3 }` | record | `(1 + 2 + 3) / 3` | `2` |

Each arm uses the pattern shape that matches its variant: no payload for `Empty`, a positional binding for `CircleR`, and named field bindings for `Rect` and `Tri`.

---

## Quick reference

```coil
// Declaration
enum E {
    Unit,                    // no payload
    Tuple(int, string),      // positional payloads
    Record { x: int, y: int }, // named fields
}

// Construction
E::Unit
E::Unit()
E::Tuple(1, "hi")
E::Record { x: 1, y: 2 }

// Matching
match value {
    E::Unit => ...,
    E::Tuple(a, b) => ...,
    E::Record { x, y } => ...,
    _ => ...,                // wildcard
}
```

---

## What's next

- [Records and Fields](04-records-and-fields.md) — field access (`p.x`), chained access (`o.x.v`), nested record patterns, and the diagnostics that guard record-shaped variants.
- [Aggregates](05-aggregates.md) — tuples, arrays, and anonymous dicts (`{ foo: 42 }`), which look similar to record variants but are a separate feature.
