# Records and Fields

Record-shaped enum variants attach **named fields** to a variant instead of positional tuple slots. You can construct them with `{ name: value }` syntax, destructure them in `match` patterns, and read individual fields with dot notation (`p.x`).

This chapter assumes you have read [Enums and Pattern Matching](03-enums-and-match.md). For unit and tuple variants, wildcards, and exhaustiveness, refer back to that chapter.

---

## Record-shaped variants

Declare a variant with named fields inside braces:

```coil
enum Point {
    Origin,
    Point { x: int, y: int },
}
```

Here `Origin` is a unit variant (no payload) and `Point` is a record variant with two `int` fields. The variant name and the enum name can be the same — `Point::Point { x: 5, y: 12 }` is normal.

---

## Constructing record values

Use `Enum::Variant { field: value, ... }`:

```coil
Point::Point { x: 5, y: 12 }
```

### Field order is flexible

Fields can appear in any order at the call site. The compiler reorders them to match the declaration:

```coil
Point::Point { y: 12, x: 5 }   // same as { x: 5, y: 12 }
```

### Nested record construction

When a field's type is another record-shaped enum, nest constructors:

```coil
enum Inner {
    Inner { v: int },
}

enum Outer {
    Outer { x: Inner, y: int },
}

let p = Outer::Outer { x: Inner::Inner { v: 42 }, y: 7 };
```

---

## Pattern matching on records

### Full field syntax

Match a record variant and bind each field:

```coil
match p {
    Point::Origin => 0,
    Point::Point { x: x_val, y: y_val } => x_val * x_val + y_val * y_val,
}
```

### Shorthand binding

When the pattern variable name matches the field name, omit the value side:

```coil
Point::Point { x, y } => x * x + y * y
```

`{ x, y }` is shorthand for `{ x: x, y: y }`.

### Nested record patterns

Patterns can nest constructors inside record fields. From `examples/nested_records.hy`:

```coil
enum Inner {
    I { v: int },
}

enum Wrap {
    W { inner: Inner, name: string },
}

fn get_v(Wrap w) -> int {
    return match w {
        Wrap::W { inner: Inner::I { v }, name } => v,
    };
}

fn main() {
    let w = Wrap::W { inner: Inner::I { v: 99 }, name: "x" };
    print "%i", get_v(w);
}
```

**Expected output:** `99`

The pattern `inner: Inner::I { v }` destructures the outer record's `inner` field and the inner record's `v` field in one step. The `name` binding is available in the arm body even though this example only uses `v`.

Nested patterns work at arbitrary depth — a record inside a tuple inside a record, and so on.

---

## Field access with dot notation

Instead of destructuring in a `match`, read a single field directly:

```coil
fn x_coord(Point p) -> int {
    return p.x;
}
```

Dot access works on values whose type is a record-shaped enum variant. The compiler resolves the field name to a slot index at compile time.

### Worked example: pattern vs. field access

From `examples/record.hy`:

```coil
enum Point {
    Origin,
    Point { x: int, y: int },
}

fn distance_squared(Point p) -> int {
    return match p {
        Point::Origin => 0,
        Point::Point { x, y } => x * x + y * y,
    };
}

fn x_coord(Point p) -> int {
    return p.x;
}

fn y_coord(Point p) -> int {
    return p.y;
}

fn main() {
    print "%i", distance_squared(Point::Point { x: 5, y: 12 });
    print "%i", x_coord(Point::Point { x: 5, y: 12 });
    print "%i", y_coord(Point::Point { x: 5, y: 12 });
}
```

**Expected output:** `169512`

| Call | Technique | Result |
|------|-----------|--------|
| `distance_squared(...)` | pattern destructuring | `5² + 12² = 169` |
| `x_coord(...)` | field access `p.x` | `5` |
| `y_coord(...)` | field access `p.y` | `12` |

Use `match` when you need to branch on the variant tag (e.g. `Origin` vs `Point`). Use dot access when you already know the shape and only need one field.

---

## Chained field access

When a field's type is itself a record-shaped enum, chain dots to read through nested records. From `examples/chained.hy`:

```coil
enum Inner {
    Inner { v: int },
}

enum Outer {
    Outer { x: Inner, y: int },
}

fn read_x_v(Outer o) -> int {
    return o.x.v;
}

fn read_y(Outer o) -> int {
    return o.y;
}

fn main() {
    let p = Outer::Outer { x: Inner::Inner { v: 42 }, y: 7 };
    print "%i", read_x_v(p);
    print "%i", read_y(p);
}
```

**Expected output:** `427`

`o.x` reads the `x` field (an `Inner` value), and `.v` reads the `v` field from that inner record. Each dot in the chain resolves against the **type of the receiver to its left**, not the outermost enum.

---

## Record variants vs. anonymous dicts

Record-shaped **enum variants** and anonymous **dict literals** look similar but are different features:

| | Enum record variant | Anonymous dict |
|---|---|---|
| Syntax | `Point::Point { x: 5, y: 12 }` | `{ x: 5, y: 12 }` |
| Type | fixed variant of a declared enum | structural `{ x: int, y: int }` |
| Tag | carries a variant tag for `match` | no tag — plain data |
| Field access | `p.x` (enum `LoadField`) | `d.x` (string-keyed lookup) |

```coil
// enum record variant — tagged, matchable
let p = Point::Point { x: 5, y: 12 };

// anonymous dict — untagged, structurally typed
let d = { x: 5, y: 12 };
```

Dicts, tuples, and arrays are covered in [Aggregates](05-aggregates.md).

---

## Diagnostics

The compiler catches common record mistakes at compile time.

### Duplicate fields

Supplying the same field twice in a constructor or pattern:

```
Duplicate field `x` in record constructor `Point`
```

```coil
Point::Point { x: 1, x: 2 }   // error
```

### Missing fields

Omitting a required field from a constructor:

```
Missing field `y` in record constructor `Point`
```

```coil
Point::Point { x: 1 }   // error — `y` is required
```

### Unknown fields

Referencing a field that does not exist in the declaration:

```
Unknown field `z` in record constructor `Point`
```

```coil
Point::Point { x: 1, y: 2, z: 3 }   // error
```

### Shape mismatch

Using the wrong payload shape for a variant:

```
Constructor `Point` payload shape mismatch (declared as record, called as tuple)
```

```coil
// declared as Point { x: int, y: int }
Point::Point(5, 12)   // error — use { x: 5, y: 12 } instead
```

### Missing field on access

Reading a field that the type does not declare:

```
Cannot find field `bar` on record `{ foo: int }`
```

This applies to both dot access on enum record variants and anonymous dicts.

---

## Quick reference

```coil
// Declaration
enum E {
    V { x: int, y: int },
}

// Construction (order flexible)
E::V { x: 1, y: 2 }
E::V { y: 2, x: 1 }

// Pattern matching
match value {
    E::V { x, y } => x + y,       // shorthand
    E::V { x: a, y: b } => a + b, // explicit
}

// Field access
value.x
value.x.nested_field   // chained

// Nested pattern
E::V { inner: Other::O { v }, label } => v
```

---

## What's next

- [Aggregates](05-aggregates.md) — tuples `(a, b)`, arrays `[1, 2, 3]`, and anonymous dicts `{ key: value }`.
- [Enums and Pattern Matching](03-enums-and-match.md) — unit and tuple variants, wildcards, exhaustiveness, and inner-pattern dispatch.
