# Chapter 2 — Types and Variables

Chapter 1 showed how to write programs; this chapter explains how zero-script **reasons about** those programs. Every expression has a type. The compiler checks types before your program runs, catching mistakes like passing a string where an integer is expected or calling an unknown function.

---

## Type annotations

Types can appear in three main places.

### On `let` bindings

```0s
let x: int = 5;
let name: string = "zero-script";
let flag: bool = false;
```

When you omit the annotation, the compiler infers the type from the initializer:

```0s
let x = 5;        // inferred as int
let pi = 3.14;    // inferred as float
```

Explicit annotations are useful when the initializer is ambiguous, when you want readable documentation, or when inference would pick a type you did not intend.

### On function parameters

```0s
fn distance(int x, int y) -> int {
    return x + y;
}
```

Each parameter requires a type name before the parameter name: `Type param`.

### On return types

```0s
fn add(int a, int b) -> int {
    return a + b;
}

fn greet() {
    print "hello";
}
```

The `-> RetType` clause is optional. Functions with no meaningful return value may omit it (they effectively return unit).

From `examples/aliases.0s`, annotations on both parameter and `let`:

```0s
type Point = (int, int);

fn distance(Point p) -> int {
    let dx = p[0];
    let dy = p[1];
    return dx + dy;
}

fn main() {
    let p: Point = (3, 4);
    print "%i", distance(p);
}
```

---

## Primitive types

zero-script provides five primitive type names used in everyday programs:

| Type     | Literal examples   | Notes                                      |
|----------|--------------------|--------------------------------------------|
| `int`    | `0`, `-42`, `100`  | Signed integer                             |
| `float`  | `1.0`, `3.14`      | IEEE-style floating point                  |
| `string` | `"hello"`          | Immutable string data                      |
| `bool`   | `true`, `false`    | Boolean                                    |
| `void`   | (no literal)       | **FFI only** — marks functions that return nothing to C/host code |

Built-in primitive names are matched **case-insensitively** at typecheck time: `int`, `Int`, and `INT` are equivalent. The same applies to `string` / `String`, etc.

`void` appears primarily in foreign-function signatures:

```0s
fn main() -> void {
    print "Hello, World!";
}
```

For ordinary zero-script functions, omit the return type instead of writing `-> void`.

---

## Type inference (Hindley–Milner)

zero-script uses a **Hindley–Milner** (Algorithm W) typechecker. You do not need to annotate every name — the compiler deduces types by analyzing how values flow through your program.

### What inference handles well

**Literals** — each literal form maps to a fixed primitive type:

```0s
let n = 42;           // int
let pi = 3.14;        // float
let msg = "hi";       // string
let ok = true;        // bool
```

**Binary operations** — operand types are unified. Adding two ints yields int; adding two floats yields float. Mixing incompatible types (e.g. `int + string`) is an error.

**Function calls** — the checker verifies arity and unifies argument types with parameter types:

```0s
fn add(int a, int b) -> int { return a + b; }

fn main() {
    let sum = add(2, 2);   // sum : int
}
```

**Return paths** — every `return expr;` in a function must agree with the declared return type (or with other return paths when inference fills in the return type).

**Polymorphism at `let`** — when the right-hand side is polymorphic (e.g. a generic helper), inference picks the most general type consistent with how the binding is *used* later in the same scope.

### When to annotate anyway

- Public or exported APIs where readers need the contract spelled out.
- Empty or minimal initializers where inference has little to go on.
- Tuple, array, and alias types where the shape matters (see [Aggregates](../tutorial/05-aggregates.md)).
- Disambiguating numeric literals when you need `float` but wrote an expression that looks integral.

Inference is not magic: it reports all type errors it finds in one pass, with source locations, so you can fix multiple issues before recompiling.

---

## Type errors and what they mean

The typechecker emits diagnostics anchored to your source. Common messages:

### `Cannot find value 'x' in this scope`

You referenced a name that was never bound, or it is out of scope (e.g. a `let` inside an inner block):

```0s
fn main() {
    print "%i", x;   // error: x not declared
}
```

**Fix:** Add `let x = ...;` above the use, or move the use into the block where `x` is defined.

### `Cannot find function 'foo'`

No function with that name is visible (typo, wrong module import, or missing declaration):

```0s
fn main() {
    bar(1);   // error if bar is not declared
}
```

**Fix:** Declare `fn bar(...) { ... }` or import the correct module (see the modules tutorial when available).

### `Type mismatch: expected 'int', found 'string'`

Two types that must be the same were incompatible:

```0s
let x: int = "hello";   // error
return "text";          // inside fn f() -> int { ... }
```

**Fix:** Change the expression, or change the annotation/return type to match reality.

### Format specifier errors

`print` and `format` validate each `%` specifier against the corresponding argument:

```0s
print "%i", "hello";   // %i requires int
print "%s", 42;        // %s requires string
print "%f", 1;         // %f requires float — use 1.0
print "%z", 1;         // %z requires bool
```

**Fix:** Use the correct specifier or convert/coerce the value to the expected type.

### `Cannot assign to undeclared variable 'x'`

Assignment (`x = expr;`) requires an existing binding from `let`:

```0s
fn main() {
    x = 10;   // error: x was never declared with let
}
```

**Fix:** `let x = 0;` before assigning.

### Assignment and immutability of binding targets

Only simple variables and certain l-values (e.g. field access on records) are valid assignment targets. Assigning to literals or malformed left-hand sides produces a targeted error with a help message.

---

## `let` binding semantics

At run time, a `let x = expr;` binding:

1. Evaluates `expr` and pushes the result on the operand stack.
2. Writes that value into **slot** reserved for `x` (via the `StorePop` instruction).
3. Advances the stack cursor so later expressions do not overwrite the slot accidentally.

This matters when you bind several variables in sequence:

```0s
let x = 5;
let y = 10;
print "%i", x + y;   // 15 — x's slot is preserved
```

**Reassignment** (`x = expr;`) uses the same slot-write path: the old value is replaced.

**Reading** a variable emits a load from that slot. The typechecker ensures you only assign types compatible with the binding's declared or inferred type.

Implications for you as a programmer:

- Each `let` introduces a **new** slot in the current frame (function activation).
- Order matters: use `let` before reading the name.
- Shadowing inner names is allowed by the parser, but inner bindings hide outer ones for the duration of the inner block — prefer distinct names when learning.

---

## Classes (brief overview)

zero-script supports class declarations and `impl` blocks for methods. From `examples/classes.0s`:

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
    let x = new Foo();
    print "%i", (2 * 2 + 3);
}
```

Key points today:

- **`class Name { field: Type, ... }`** declares fields.
- **`impl Name { fn method(...) -> Ret { ... } }`** attaches methods.
- **`new Foo()`** allocates an instance on the heap.

Field access and mutation on classes are still limited compared to record-shaped enums and dict literals. Prefer enums with record payloads or `{ key: value }` records for data-oriented code until the classes tutorial is expanded.

**Coming soon:** dedicated coverage of classes, `impl`, and instance field access — see `examples/classes.0s` for the current syntax snapshot.

---

## Type aliases

Give a readable name to an existing type:

```0s
type Point = (int, int);

fn length(Point p) -> int {
    return p[0] + p[1];
}
```

Aliases are **purely compile-time**: they expand during typechecking and have **zero run-time cost**. They do not create a new distinct type in the nominal sense — two names for the same structure unify structurally.

Rules of thumb:

- Declare aliases at the top level: `type Name = T;`
- The right-hand side can be any type annotation form: primitives, tuples, arrays, fixed arrays, or class names.
- Later declarations with the same name overwrite earlier ones (no scoped aliases yet).

Full treatment of aliases alongside tuples and arrays appears in [Chapter 5 — Aggregates](../tutorial/05-aggregates.md). See `examples/aliases.0s` for a runnable example.

---

## Putting it together

A small program that combines annotations, inference, and checked output:

```0s
fn clamp(int lo, int hi, int v) -> int {
    if v < lo {
        return lo;
    }
    if v > hi {
        return hi;
    }
    return v;
}

fn main() {
    let x = clamp(0, 100, 150);   // inferred int
    print "%i", x;                 // 100
    print "%z", x == 100;          // true
}
```

If you change `clamp` to return `"100"` (string), the checker reports a `Type mismatch` on `return` before you run the program.

---

## Exercises

1. Add explicit types to every binding in `examples/let_test.0s` without changing behavior.

2. Write a function `max(int a, int b) -> int` and a `main` that prints the result. Remove the return type and confirm inference still accepts the program.

3. Introduce deliberate errors (one at a time) and record the diagnostic text:
   - unknown variable
   - wrong `print` specifier
   - assignment without `let`

4. Declare `type UserId = int;` and write `fn fetch(UserId id) -> int` that returns `id * 2`. Call it from `main`.

5. Read `examples/classes.0s` and explain (in comments or a note) which operations are type-checked but not yet fully supported at run time.

---

## See also

- [Chapter 1 — Basics](01-basics.md) — syntax, control flow, and `print`
- [Chapter 5 — Aggregates](../tutorial/05-aggregates.md) — tuples, arrays, records, and aliases in depth
- [Operator reference](../reference/operators.md) — operators and operand types
- `examples/let_test.0s` — binding and reassignment
- `examples/aliases.0s` — type alias end-to-end
- `examples/classes.0s` — class syntax snapshot
