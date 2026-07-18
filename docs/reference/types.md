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

Strings support `+` / `+=` with other strings. The `format` expression returns `string` and uses the same specifier checks as `print`.

---

## Type constructors (`Ty::Con`)

Any identifier that is not a primitive becomes an opaque type constructor. User-defined **class** names also register as `Ty::Con(name)`.

Recursive enum references use isorecursive `Con(name)` inside variant payloads (not unfolded `Sum`), so recursive types like `Tree` are expressible without infinite-type errors during inference.

---

## Built-in `Option` and `Result`

The compiler pre-registers polymorphic sum types (same mechanism as `FFIType`). **Do not redeclare them** — a user `enum Option` / `enum Result` is a duplicate-enum error.

| Type | Variants (tag order) | Annotation |
|------|----------------------|------------|
| `Option` | `None` (0), `Some(T)` (1) | `Option` / `Option<T>` |
| `Result` | `Ok(T)` (0), `Err(E)` (1) | `Result` / `Result<T, E>` |

Payload types are inferred at use sites (`Option::Some(1)` → `Option` of `int`). Error-handling operators (`raise`, `?`, `??`, `?.`) are documented in [Operators](operators.md) and [Tutorial 09](../tutorial/09-error-handling.md).

**Result mode:** a function that uses `raise` or Result-`?` has return type `Result<T, E>`; success `return` values are implicitly wrapped as `Ok`. One `E` per function.

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
enum Tree {
    Leaf,
    Node(int, Tree, Tree),
}
```

Internal shape (illustrative):

```
Ty::Sum {
    name: "Tree",
    variants: [
        ("Leaf", Unit),
        ("Node", Tuple([int, Con("Tree"), Con("Tree")])),
    ],
}
```

### Generic enums (`Ty::App`)

User enums may take type parameters. Annotations and construct/match use the same `Ty::App` machinery as builtin `Option` / `Result`:

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
```

`Box::Full(7)` has type `Box<int>`. Payload types are freshened per construct/match site from the enum's schema (type-param placeholders in the registry).

### Variant payload shapes (`EnumVariantPayloadTy`)

| Shape | Syntax example | Internal |
|-------|----------------|----------|
| Unit | `None` or `None()` | `Unit` |
| Tuple | `Some(int)` | `Tuple([int])` |
| Record | `Point { x: int, y: int }` | `Record([("x", int), ("y", int)])` |

Constructors in expressions and patterns use qualified form: `Option::Some(42)` (builtin), `Point::Point { x: 1, y: 2 }` (user enum).

### Constructor types (`Ty::Constructor`)

Applying a variant yields a constructor type carrying tag and arity, unified against the parent sum (or applied `Ty::App` for polymorphic enums).

---

## Tuples (`Ty::Tuple`)

Heterogeneous fixed-length products:

```0s
let t = (1, "hi", true);   // (int, string, bool)
fn pair(int a, string b) -> (int, string) { return (a, b); }
```

Annotation: `(T1, T2, ...)`. Literal syntax requires a comma: `(1,)` is a 1-tuple; `(1)` is a parenthesized expression.

Tuples have structural `Show` support for `%v` when every element is showable. The printed form is `(a, b)` (and `(a,)` for a 1-tuple).

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

### Growing arrays

Use `push(arr, value)` to append in place. The value must match the array's element type. The call returns the same array as a dynamic `[T]`, and `len(arr)` returns its current runtime length as `int`.

```0s
let xs = [1, 2];   // starts as [int; 2]
push(xs, 3);       // xs is treated as dynamic [int] afterwards
print "%i", len(xs);
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
- Anonymous records have structural `Show` support for `%v` when every field is showable. Fields print in canonical name order as `{ a: 1, b: 2 }`.

Structural `Show` is limited to tuples and anonymous records. Enums, classes, and other user types still need an explicit `impl Show<T>`.

---

## Type aliases (`type Name = T;`)

Substituted at typecheck time; zero runtime cost. Parametric aliases expand when applied:

```0s
type UserId = int;
type IntPair = (int, int);
type Pair<T> = (T, T);

fn id(UserId x) -> UserId { return x; }

fn main() {
    let p: Pair<int> = (3, 4); // same as `(int, int)`
}
```

| Property | Behavior |
|----------|----------|
| Scope | Lexical: program, function, and block scopes |
| Shadowing | Inner scopes may shadow outer aliases |
| Duplicate names | Duplicate alias in the same scope is a diagnostic |
| RHS | Any `type_annotation` form |
| Type parameters | `type Pair<T> = (T, T);` — `Pair<int>` expands to `(int, int)` |

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

## Coroutine types (`coroutine<Y, S>`)

`async fn` bodies return a handle typed as `coroutine<Y, S>`:

| Parameter | Meaning |
|-----------|---------|
| `Y` | Type **yielded out** on each `yield expr` |
| `S` | Type **sent in** on `resume h with v` and received by `let x = yield e` |

When no binding-yield or send sites exist, `S` defaults to `unit` and diagnostics print `coroutine<Y>`.

```0s
async fn counter() -> coroutine<int> {
    yield 0;
    yield 1;
}

async fn ping() -> coroutine<string, string> {
    let msg = yield "ready";
    yield msg;
}
```

Resume expression type: if `h : coroutine<Y, S>`, then `resume h` has type `Y`, and `resume h with v` requires `v : S`.

`resume` has a single static result type (`Y`) covering BOTH the value
yielded by each `yield expr;` AND the value produced when the body
completes (`return expr;`, or falling off the end). A `return expr;`
inside an `async fn` therefore unifies `expr`'s type against the SAME
`Y` as every `yield` in that body — not `unit` — so the returned value
is not discarded:

```0s
async fn counter() {
    yield 1;
    yield 2;
    return 42; // completion value, type unifies with the `yield`s above
}

fn main() {
    let h = counter();
    resume h; // 1
    resume h; // 2
    resume h; // 42 (the `return` value)
    resume h; // 0  (Done — see below, NOT 42 again)
}
```

Resuming an already-`Done` coroutine always yields `Value::default()`
(`0`/equivalent), never the coroutine's last `return` value — there is
no error-handling protocol yet to signal "resumed after completion",
so a fixed sentinel keeps the behavior well-defined instead of leaking
a stale value.

---

## Generics and typeclasses

Generic functions use an optional type-parameter list and typeclass bounds on parameters:

```0s
fn add<T: Num>(T a, T b) -> T { return a + b; }

fn main() {
    print "%i", add(3, 4);   // int
    print "%f", add(1.5, 2.5); // float
}
```

Multi-parameter typeclasses use a trailing `where` clause:

```0s
typeclass Convert<A, B> { fn cast(A x) -> B; }
impl Convert<int, int> { fn cast(int x) -> int { return x; } }
fn apply_cast<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
```

Binder bounds (`T: Num`) remain the short form for unary classes; they desugar to
the same constraint shape as `where Num<T>`.

### Syntax

| Form | Meaning |
|------|---------|
| `fn id<T>(T x) -> T` | Unconstrained type parameter `T` |
| `fn add<T: Num>(T a, T b) -> T` | `T` must satisfy the `Num` bound |
| `fn both<T: Num + Eq>(T x) -> T` | Multiple bounds (`+`) |
| `fn f<A, B>(A x) -> B where Convert<A, B>` | Multi-param (or unary) `where` constraint |
| `typeclass Container<F: * -> *>` | Unary type-constructor parameter |
| `typeclass Bifunctor<F: * -> * -> *>` | Binary type-constructor parameter |
| `typeclass Higher<F: (* -> *) -> *>` | Higher-order constructor parameter |
| `fn f<F: * -> * -> *, Bifunctor, A, B>(F<A, B> x)` | Explicit kind plus a class bound on one parameter |

Call-site strategy:

| Situation | Runtime |
|-----------|---------|
| Ground call with only **builtin** bounds (`Num`/`Ord`/`Eq`/`Show`) | May **monomorphize** into a specialized clone (unboxed `ADD`, etc.) |
| Shared body / open type params with any bound | **Dictionary passing** — see below |
| Ground or shared call with user typeclass bounds | **Dictionary passing** — see below |
| Escaped generic fn value (`let f = id;`) | `MakePolyFn` / `MakePolyFnCapture` + `CallIndirect` |

### Dictionary passing

Constrained calls that are not monomorphized append one dictionary per typeclass constraint after the value arguments. Each dictionary is a `MakeTuple` of method code offsets in declaration order. The callee reserves trailing locals `__dict0`, `__dict1`, …, loads the matching method slot with `Index`, and invokes it with `CallIndirect`. A generic calling another generic with the same open bound forwards its existing dictionary. Builtin classes use compiler-generated primitive method thunks through this same ABI; ground monomorphization remains an optimization.

**Flattened superclass layout.** When a unary class declares a param bound
(`typeclass Ordered<T: Equal>`), those bounds are stored as *superclasses*.
The runtime dictionary for the subclass is flattened: subclass methods first,
then each superclass’s methods in declaration order (transitively). An
`impl Ordered<int>` therefore requires an existing `Equal<int>` instance — its
methods fill the trailing dict slots.

```0s
typeclass Describable<T> { fn describe_val(T x) -> int; }
impl Describable<int> { fn describe_val(int x) -> int { return x + 1; } }
fn show<T: Describable>(T x) -> int { return x.describe_val(); }
// show(42) → CALL arity = 2 (value + Describable dict)
```

Bound methods support both equivalent forms:

```0s
x.describe_val(); // method sugar
describe_val(x);  // bare / UFCS form
```

Default methods occupy normal dictionary slots. Every implementation method
receives the active dictionary as a hidden trailing argument, so a default can
call a sibling method. An omitted default slot points at the class default body.

### Builtin typeclasses

The compiler pre-registers these typeclasses and instances for `int`, `float`, and (where applicable) `string`:

| Class | Purpose | Operators / methods |
|-------|---------|---------------------|
| `Num` | Arithmetic | `+`, `-`, `*`, `/` (and `%` for ints) |
| `Ord` | Ordering | `<`, `<=`, `>`, `>=` |
| `Eq` | Equality | `==`, `!=` |
| `Show` | Display | `show(T) -> string`; used by format `%v` |

Calling `add<T: Num>(…)` with `string` arguments is a type error when no `Num<string>` instance applies to that use (string `+` is only available through the `Num` instance wiring).

### User-defined typeclasses (sketch)

Declare a class and provide instances for concrete types:

```0s
typeclass Measurable<T> {
    fn size(T x) -> int;
}

impl Measurable<int> {
    fn size(int x) -> int { return x; }
}
```

Instance methods compile to ordinary functions with mangled names
(`Class__Type__method`). Generic call sites discharge the bound at
typecheck time and pass the matching dictionary at runtime (above).

### Instance coherence

Typeclass instances follow module-path ownership rules so dictionary
resolution stays deterministic across projects:

- `impl Class<T…>` is allowed when the current module defines `Class`.
- Otherwise, every non-variable instance argument must have a nominal
  head (enum, class, or type alias) defined in the current module.
- Builtin types (`int`, `float`, `string`, tuples, arrays, and records)
  are not local nominal heads. For example, a module that did not define
  `Show` cannot add `impl Show<(int, int)>`.
- Exact duplicates and instances whose heads unify with an existing
  instance are rejected.
- If constraint discharge ever sees two matching instances, it reports
  an ambiguous-instance error rather than selecting the first one.

### Associated types and GATs

A typeclass may declare associated types; each impl must define them. Associated
types may be nullary (`type Elem;`) or generic (`type Ref<T>;`, also called a
generic associated type / GAT):

```0s
typeclass Collect<C> {
    type Elem;
    fn head(C xs) -> Elem;
}

impl Collect<Option<int>> {
    type Elem = int;
    fn head(Option<int> xs) -> int {
        return match xs {
            Option::Some(v) => v,
            Option::None => 0,
        };
    }
}
```

- **In method signatures** inside the class, bare `Elem` (and `Collect::Elem` /
  `C::Elem`) resolve to the associated type. Method schemes quantify class
  parameters first, then any associated-type projection variables.
- **Impls** must define every associated type (`type Elem = …;`) and may not
  introduce unknown ones. Missing or extra assoc types are type errors. A GAT
  definition repeats its own binders, for example `type Ref<T> = T;`.
- **Projections** `Owner::Assoc` and applied GAT projections
  `Owner::Assoc<T, U>` are allowed in type annotations. When the
  owner is a type parameter with an active class bound that declares the
  assoc type (`fn take_head<C: Collect>(C xs) -> C::Elem`), the projection
  is an open type variable that is pinned when a ground instance is
  discharged at the call site (`take_head(Option::Some(42))` → `int`).
- **GAT arguments are kind-checked.** `type Ref<F: * -> *>;` requires applied
  projections such as `P::Ref<Option>` to pass a constructor-kinded argument,
  while `P::Ref<int>` is rejected.

```0s
typeclass Pointer<P: * -> *> {
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

Associated types are erased at runtime (no dictionary slot); they exist only
in the typechecker. See `examples/assoc_type.0s` and
`examples/gat_pointer.0s`.

### Superclasses and implied bounds

Unary typeclass parameter bounds declare superclasses:

```0s
typeclass Equal<T> { fn eq_val(T a, T b) -> bool; }
typeclass Ordered<T: Equal> { fn lt_val(T a, T b) -> bool; }

impl Equal<int> { fn eq_val(int a, int b) -> bool { return a == b; } }
impl Ordered<int> { fn lt_val(int a, int b) -> bool { return a < b; } }

// Implied Equal: no need to write `T: Ordered + Equal`
fn cmp_eq<T: Ordered>(T a, T b) -> bool {
    return eq_val(a, b);
}
```

- **Impl check:** `impl Ordered<int>` errors unless `Equal<int>` already exists.
- **Implied bounds:** an active constraint `Ordered<T>` covers `Equal<T>` for
  discharge and method resolution, so superclass methods are available under
  the subclass bound alone.
- **Dict slots:** `Ordered` dict = `[lt_val, eq_val]` (subclass then superclass).

Builtin `Ord` / `Eq` are independent (no superclass link) so existing builtin
dict layouts stay unchanged. Prefer a custom `Ordered` / `Equal` pair when you
need superclass semantics. See `examples/superclass_ord.0s`.

### First-class generic functions

A generic function can escape into a local `PolyFn` value and be instantiated
more than once:

```0s
fn id<T>(T x) -> T { return x; }
let f = id;
let n = f(42);
let x = f(4.0);
```

Unconstrained escapes use `MakePolyFn`. Constrained generics always escape via
`MakePolyFnCapture`: each constraint slot is filled from an in-scope `__dictN`
or a concrete instance dictionary when the type arguments are ground; only
truly unavailable evidence (for example top-level `let f = show;`) leaves a
null slot for application-time synthesis. Applications use `CallIndirect`,
which merges captured evidence with any dictionaries synthesized at the call
site (preferring captures for already-filled slots). A generic identifier
passed to a compatible `forall T. T -> T` parameter uses the same path.

### Higher-rank `forall`

Type annotations may use prenex / higher-rank quantification:

```0s
fn app(forall T. T -> T f, int x) -> int {
    return f(x);
}
```

`forall T: Num. …` carries constraints on the binder. When checking an
argument against a `forall` expectation, the checker skolemizes the
binder (rigid variables) and rejects escaping skolems. A polymorphic
generic function identifier (e.g. `id`) is compatible with a matching
`forall` parameter type.

See `examples/generics.0s`, `examples/typeclass_dict.0s`,
`examples/superclass_ord.0s`, `examples/assoc_type.0s`,
`examples/gat_pointer.0s`, and `examples/polyfn.0s` for runnable demos.

### Boxing and unboxing at generic boundaries

When a concrete value crosses into a generic function body, the compiler wraps it in a heap-allocated `ObjBoxed` cell (`BoxValue`). When the generic call returns a value whose type is concrete at the call site, the compiler immediately unpacks it back to a raw value (`UnboxValue`).

This means **most generic calls to primitive-returning functions are transparent** — the caller receives a plain `int`, `float`, `bool`, or `string`, not a boxed wrapper:

```0s
fn id<T>(T x) -> T { return x; }

fn main() {
    let n = id(42);   // n is a raw int — unboxed automatically
    print "%i", n;    // prints: 42
}
```

**Displaying open / generic values — use `%v`:**

Concrete format specifiers (`%i`, `%f`, `%s`, `%z`, …) require a resolved concrete type. An open type parameter is a type error; use `%v`, which requires `T: Show` and lowers through the `show` method to a string before formatting:

```0s
fn show_it<T: Show>(T x) {
    print "%v", x;   // ok — dictionary Show
}

fn main() {
    show_it(42);
    show_it("hi");
    let s = format "%v", 99;  // same lowering; leaves a string
    print "%s", s;
}
```

Builtin `Show` instances cover `int`, `float`, `string`, `bool`, and `unit`. User types can `impl Show<MyType>`. See `examples/generic_print.0s`.

---

## Known limitations

| Area | Limitation |
|------|------------|
| Type aliases | Lexically scoped (stack of frames); duplicate names in the same frame are rejected; inner scopes may shadow outer; parametric aliases (`type Pair<T> = …`) expand on application |
| Classes | Nominal `Ty::Con`; ctor args / fields / methods supported — no inheritance or virtual dispatch |
| FFI | Broad scalar/Ptr/struct/callback tags via `FFIType` / `extern struct` — see [FFI tutorial](../tutorial/07-ffi.md) |
| Generics | Generic **functions** / enums / aliases with type params and `T: Class` bounds; builtin `Option`/`Result` and user `enum Box<T>` as `Ty::App` (construct/match freshen payloads); builtin `Num`/`Eq`/`Ord`/`Show`; user `typeclass`/`impl` with dictionary passing; `forall` rank-n annotations; mono for ground builtin-bound calls |
| Higher-kinded types | Constructor kinds such as `F: * -> *`, `F: * -> * -> *`, and `F: (* -> *) -> *`; kind variables are not supported |
| Effect system | No linear/ownership types |
| Callback returns | Opaque `Ptr` address; re-invoke requires host/`declare` of the pointed-to symbol (no automatic trampoline) |
| Chained field access | Typechecker validates; codegen uses side-table for simple receivers |
| Inner match patterns | Same outer tag with different inner tags — supported (Phase 18A); complex nested cases may still need careful arm ordering |
| `async fn` `-> T` annotation | When present, `T` is unified with the coroutine yield/return type `Y` (same slot as `yield` / `return` / `resume`). A mismatch is a type error. |

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
