# Types reference

zero-script uses **Hindley–Milner (Algorithm W)** type inference with optional annotations. Types are checked once per program before codegen; the compiler caches `(NodeId → Ty)` for opcode selection (e.g. `ADD` vs `ADDF`).

---

## Primitive types

| Name | `Ty` representation | Notes |
|------|---------------------|-------|
| `int` | `Ty::Con("int")` | 64-bit signed integer at runtime |
| `float` | `Ty::Con("float")` | IEEE double |
| `string` | `Ty::Con("string")` | Heap-allocated UTF-8 string |
| `bool` | `Ty::Con("bool")` | `true` / `false` |
| `void` / `unit` | `Ty::Con("unit")` | Used for statements with no value; FFI `void` return |

Primitive names in annotations are matched **case-insensitively** (`Int` ≡ `int`).

---

## Type constructors (`Ty::Con`)

Any identifier that is not a primitive becomes an opaque type constructor:

```0s
enum Option { None, Some(int) }
// Option is Ty::Con("Option") when referenced before full sum info
```

User-defined **class** names also register as `Ty::Con(name)`.

Recursive enum references use isorecursive `Con(name)` inside variant payloads (not unfolded `Sum`), so recursive types like `Tree` are expressible without infinite-type errors during inference.

---

## Function types (`Ty::Fun`)

Curried internally: `int -> int -> int` means `(int, int) -> int`.

```0s
fn add(int a, int b) -> int { return a + b; }
// add : int -> int -> int
```

---

## Sum types / enums (`Ty::Sum`)

Declared with `enum Name { variants }`:

```0s
enum Result {
    Ok(int),
    Err(string),
}
```

Internal shape:

```
Ty::Sum {
    name: "Result",
    variants: [
        ("Ok", Tuple([int])),
        ("Err", Tuple([string])),
    ],
}
```

### Variant payload shapes (`EnumVariantPayloadTy`)

| Shape | Syntax example | Internal |
|-------|----------------|----------|
| Unit | `None` or `None()` | `Unit` |
| Tuple | `Some(int)` | `Tuple([int])` |
| Record | `Point { x: int, y: int }` | `Record([("x", int), ("y", int)])` |

Constructors in expressions and patterns use qualified form: `Option::Some(42)`, `Point::Point { x: 1, y: 2 }`.

### Constructor types (`Ty::Constructor`)

Applying a variant yields a constructor type carrying tag and arity, unified against the parent sum.

---

## Tuples (`Ty::Tuple`)

Heterogeneous fixed-length products:

```0s
let t = (1, "hi", true);   // (int, string, bool)
fn pair(int a, string b) -> (int, string) { return (a, b); }
```

Annotation: `(T1, T2, ...)`. Literal syntax requires a comma: `(1,)` is a 1-tuple; `(1)` is a parenthesized expression.

---

## Arrays (`Ty::Array`)

Homogeneous collections with optional static length:

| Annotation | `ArrayLength` | Example |
|------------|---------------|---------|
| `[T]` | `Dynamic` | Function param — length unknown at compile time |
| `[T; N]` | `Static(N)` | Literal `[1, 2, 3]` infers `[int; 3]` |

```0s
let xs = [1, 2, 3];        // [int; 3]
fn sum([int] arr) -> int { /* ... */ }  // dynamic length param
```

### Indexing

| Target | Compile-time index | Runtime index |
|--------|-------------------|---------------|
| Static array `[T; N]` | OOB literal → diagnostic | Allowed (no static check) |
| Dynamic `[T]` | N/A | Allowed (no OOB diagnostic) |
| Tuple | OOB literal → diagnostic | — |
| Non-aggregate | Error | — |

---

## Records / dicts (`Ty::Record`)

Anonymous structurally typed records:

```0s
let d = { x: 1, y: 2 };   // { x: int, y: int }
let n = d.x;              // field access
```

- Two record literals with the same field names and compatible field types unify structurally.
- Field access on records uses string-keyed `GetField`; enum record variants use index-based `LoadField`.
- Duplicate field names in one literal → compile error.

---

## Type aliases (`type Name = T;`)

Substituted at typecheck time; zero runtime cost.

```0s
type UserId = int;
type IntPair = (int, int);

fn id(UserId x) -> UserId { return x; }
```

| Property | Behavior |
|----------|----------|
| Scope | Global per compilation unit (no block scoping yet) |
| Shadowing | Later alias with same name overwrites |
| RHS | Any `type_annotation` form |

---

## Type annotation syntax (all contexts)

| Context | Example |
|---------|---------|
| Function parameter | `fn f(int x, [string] rows) -> bool` |
| Return type | `-> (int, int)` |
| `let` binding | `let x: int = 1;` |
| Enum variant payload | `Some(int)`, `Node { left: Tree, right: Tree }` |
| Type alias RHS | `type A = [int; 4];` |
| Class field | `name: string` |

Forms:

```
IDENT                    // int, MyEnum, Foo
'[' IDENT (';' INT)? ']' // [T] or [T; N]
'(' type (',' type)+ ')'  // tuples — at least two components in type position
```

---

## Inference highlights

| Feature | Behavior |
|---------|----------|
| Let-polymorphism | `let`-bound names generalize free type variables at binding site |
| Function recursion | Monomorphic recursion — `fn` body sees monomorphic self type |
| `match` exhaustiveness | Checked post-inference; non-exhaustive match → diagnostic |
| Format strings | `print "%i", x` validates specifier vs argument type |
| `impl` methods | `self` is implicit first parameter of owner class type |

---

## Unification rules (summary)

Unification is structural (Robinson) with an occurs check.

| Left | Right | Result |
|------|-------|--------|
| Same `Ty::Var` | | Success |
| Same `Ty::Con` name | | Success |
| `Ty::Con(n)` | `Ty::Sum { name: n, .. }` | Isorecursive expand-and-unify |
| `Ty::Fun` | `Ty::Fun` | Unify args, then returns |
| `Ty::Tuple` | `Ty::Tuple` | Same length; unify each element |
| `Ty::Array` | `Ty::Array` | Unify elements; lengths compatible if either is `Dynamic` or both `Static` with same N |
| `Ty::Record` | `Ty::Record` | Same fields (sorted by name); unify each field type |
| `Ty::Sum` | `Ty::Sum` | Same enum name; same variant names, shapes, arities; unify payload types |
| `Ty::Constructor` | `Ty::Sum` / other constructor | Tag, arity, owner must match |
| `Ty::Var` | anything | Bind variable (if occurs check passes) |
| Otherwise | | `Type mismatch` error |

### Length compatibility (arrays)

```
Dynamic  ~  Static(N)   ✓
Dynamic  ~  Dynamic     ✓
Static(N) ~ Static(M)    ✓ (element types must unify; N and M need not match for unification of annotation vs literal in all cases — mismatched static lengths error when both static)
```

---

## Known limitations

| Area | Limitation |
|------|------------|
| Type aliases | No lexical scoping; duplicate names silently overwrite |
| Records | `SetField` mutation is limited — prefer fresh `MakeDict` values for reliable semantics |
| Classes | Nominal typing partial; limited runtime method dispatch |
| FFI | Only `int`, `float`, `string`, `void` — see [FFI tutorial](../tutorial/07-ffi.md) |
| Generics | No user-defined generic types |
| Higher-kinded types | Not supported |
| Effect system | No linear/ownership types |
| `invoke` typing | Result type not refined from `declare` signature |
| Chained field access | Typechecker validates; codegen uses side-table for simple receivers |
| Inner match patterns | Same outer tag with different inner tags — supported (Phase 18A); complex nested cases may still need careful arm ordering |
| String `+` | Not in current tree — do not rely on string concatenation |
| `const` | No `const` keyword in parser — use `let` |

---

## Pretty-printed forms

Diagnostic messages render types roughly as:

| Internal | Display |
|----------|---------|
| `int` | `` `int` `` |
| `(int, string)` | `` `(int, string)` `` |
| `[int]` | `` `[int]` `` |
| `[int; 5]` | `` `[int; 5]` `` |
| `{ x: int, y: int }` | `` `{ x: int, y: int } `` |
| `Option` sum | `` `Option` `` with variant detail in specialized errors |

---

## Related documents

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Where annotations appear in grammar |
| [Operators](operators.md) | Arithmetic and comparison typing |
| [Built-ins](built-ins.md) | FFI type tags |
| [Tutorial: Types](../tutorial/02-types-and-variables.md) | Guided introduction |
