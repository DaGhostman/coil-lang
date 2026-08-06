# Operators reference

coil expressions use a **Pratt parser** with prefix, infix, and postfix operators. Higher rows in the [precedence table](#precedence-table-high-to-low) bind tighter.

Associativity:

| Class | Operators | Associativity |
|-------|-----------|---------------|
| Additive `+` `-` | Term level | **Left** |
| Most other binary | | **Right** |
| Coalesce `??` | Between `\|\|` and assignment | **Right** |
| Assignment `=` | | **Right** |
| Postfix `++` `--` `.` `?.` `[]` `?` | | N/A (postfix) |
| Prefix `-` `+` `~` | | N/A (prefix) |

---

## Precedence table (high to low)

| Precedence | Operators / forms | Notes |
|------------|-------------------|-------|
| **Primary (postfix)** | `expr++`, `expr--`, `expr.field`, `expr?.field`, `expr[index]`, `expr?` | Tightest — postfix on atoms |
| **Call (postfix)** | `f(args)` | Function / method call |
| **Cast (postfix)** | `expr as T` | Binds tighter than `*` / `+` and assignment (`c = m as byte` → RHS cast) |
| **Prefix unary** | `-expr`, `+expr`, `~expr` | Numeric negation, no-op plus, bitwise NOT |
| **Exponentiation** | `**` | Right-associative |
| **Multiplicative** | `*`, `/`, `%` | Right-associative |
| **Additive** | `+`, `-` | **Left**-associative; operands must unify to same type |
| **Bit shift** | `<<`, `>>` | |
| **Bitwise AND** | `&` | |
| **Bitwise XOR** | `^` | Bitwise, not logical |
| **Bitwise OR** | `\|` | |
| **Logical AND** | `&&` | Both operands `bool` → `bool` |
| **Logical OR** | `\|\|` | Both operands `bool` → `bool` |
| **Comparison** | `==`, `!=`, `<`, `<=`, `>`, `>=` | Operands same type → `bool` |
| **Range** | `..`, `..=` | Non-associative; bounds unify to `T: Ord` → lazy `Range<T>` / `RangeInclusive<T>` |
| **Coalesce** | `??` | Right-associative; Option / Result only (see below) |
| **Assignment** | `=`, `+=`, `-=`, … | Lowest — right-associative |

Forms **not** in the Pratt table but still tight-binding:

| Form | Binding |
|------|---------|
| Qualified construct `E::V(...)` | Atom |
| Grouping `(expr)` | Atom |
| `match`, `new`, literals | Atoms |

---

## Arithmetic

| Operator | Types | Result | VM op (int / float) |
|----------|-------|--------|---------------------|
| `+` | `int` / `float` (both same), or `string` + `string` | same | `ADD` / `ADDF`; strings lower through `FORMAT "%s%s"` |
| `-` | `int` / `float` | same | `SUB` / `SUBF` |
| `*` | `int` / `float` | same | `MUL` / `MULF` |
| `/` | `int` / `float` | same | `DIV` / `DIVF` |
| `%` | `int` / `float` | same | `MOD` / `MODF` |
| `**` | `int` / `float` | same | `Pow` / `PowF` |

Mixed `int` and `float` operands → **type error** at compile time.

When the factor is a **compile-time power of two** (`2`, `4`, `8`, …), `int` /
`byte` multiplication lowers to a left shift (`SHL` / `<<`) instead of `MUL`.
Trait/`Mul` dictionary dispatch and float `*` are never rewritten this way.

### Aggregate (vector) arithmetic

Homogeneous numeric tuples and arrays support the same operators
**element-wise**, plus scalar broadcast:

| Left | Right | Result |
|------|-------|--------|
| `(T,…,T)` | `(T,…,T)` (same arity) | zip |
| `[T; N]` | `[T; N]` | zip |
| `[T]` | `[T]` / `[T; N]` | **hard error** (length not known at compile time) |
| aggregate | scalar `T` (or reverse) | broadcast |

`T` must be numeric (`int` / `float` / `byte`, or a `Num`-bounded type
parameter).
`*` and `**` are element-wise (not dot product / matrix power). Unary `-`
negates each element. Compound assign (`+=`, `**=`, …) follows the same
rules with the LHS shape fixed.

Static-length zip/broadcast uses a packed HostInvoke SIMD kernel when the
fixed length is ≥ 8 (`packed_vec_arith`); smaller shapes still unroll to
scalar opcodes (bytecode size scales with `N` in that case).

For **dot product**, **cross product**, and bare-array **matrix multiply**,
use the named helpers `dot`, `cross`, and `matmul`. For matmul via `*`
(Mul), wrap rows with `matrix(...)` to get a nominal `Matrix` — see
[Built-ins](math.md).

```coil
(1, 1) + (1, 1);   // (2, 2)
[1, 2] + 3;        // [4, 5]
-(1, 2);           // (-1, -2)
dot((1, 2), (3, 4));  // 11
let a = matrix([[1, 2], [3, 4]]);
let b = matrix([[5, 6], [7, 8]]);
a * b;             // matmul (Matrix)
a + a;             // element-wise
```

See `examples/vec_tuple.hy`, `examples/vec_array.hy`,
`examples/vec_generic.hy`, `examples/vec_packed_mul.hy`, `examples/vec_dot.hy`,
`examples/vec_matmul.hy`, and `examples/matrix_mul.hy`.

String concatenation uses `+`:

```coil
let s = "hello" + " " + "world";
s += "!";
```

Mixing `string` with a non-string operand is a compile-time type error.

---

## Bitwise

Operands are inferred together (typically `int`):

| Operator | Meaning |
|----------|---------|
| `&` | Bitwise AND |
| `\|` | Bitwise OR |
| `^` | Bitwise XOR |
| `<<` | Shift left |
| `>>` | Shift right |
| `~` | Bitwise NOT (prefix) |

---

## Logical

| Operator | Operands | Result |
|----------|----------|--------|
| `&&` | `bool`, `bool` | `bool` |
| `\|\|` | `bool`, `bool` | `bool` |

Short-circuit behavior follows VM evaluation order (both operands evaluated eagerly in current codegen).

| Operator | VM opcode |
|----------|-----------|
| `&&` | `AND` |
| `\|\|` | `OR` |
| `&` | `BITAND` |
| `\|` | `BITOR` |
| `^` | `XOR` |
| `<<`, `>>` | `SHL`, `SHR` |

---

## Comparison

| Operator | Operands | Result |
|----------|----------|--------|
| `==`, `!=` | Same type | `bool` |
| `<`, `<=`, `>`, `>=` | Same type (`int` or `float`) | `bool` |

Float and int comparisons use separate opcode families (`LE` vs `LEF`, etc.) selected at codegen from inferred types.

`==` / `!=` on arrays and tuples are **structural** (length + element-wise, recursively). Strings compare by UTF-8 content. Enums, classes, and records stay pointer-identity unless a user `Eq` instance is dispatched.

---

## Assignment

```
identifier = expr
field_access = expr
array[index] = expr
identifier += expr    // and other compound forms
```

| Rule | Detail |
|------|--------|
| LHS | Identifier, dict field (`d.x`), or array index (`arr[i]`) |
| Compound | `+=`, `-=`, `*=`, `/=`, `%=`, `**=`, `<<=`, `>>=`, `&=`, `\|=`, `^=` |
| Type | RHS must unify with the assigned slot (bitwise compound ops require `int`) |
| Undeclared | Error — use `let` first |
| Value | Assignment and compound-assignment expressions evaluate to the assigned value |

`let` bindings use `STORE` (pop TOS into the slot); match arms need no store opcode for pattern slots (value already placed by unpack/match).

---

## Increment / decrement

| Form | Syntax | Result value |
|------|--------|--------------|
| Postfix increment | `expr++` | Old value |
| Postfix decrement | `expr--` | Old value |
| Prefix increment | `++expr` | New value |
| Prefix decrement | `--expr` | New value |

Works on variables, mutable dict fields, and array elements. Enum record fields and tuples are immutable.

---

## Field access (`.field`)

| Form | Example | Precedence |
|------|---------|------------|
| Postfix dot | `p.x`, `p.x.y` | Primary — left-to-right |

Binds like Rust/C:

```
a.b.c  →  Access(Access(a, "b"), "c")
t[i].x →  Access(Index(t, i), "x")
```

Float literals remain atoms: `1.0` is not `1.x`.

Field resolution:

| Receiver type | Mechanism |
|---------------|-----------|
| Enum record variant | `LoadField` (index by declaration order) |
| Dict / `{ }` record | `GetField` (string key) |

---

## Error-handling operators (`?`, `?.`, `??`)

Desugared to `match` / `return` / `MakeEnum` — no new opcodes. See [Tutorial: Error handling](../manual/tutorial/09-error-handling.md).

| Operator | Form | Operand | Result type | Notes |
|----------|------|---------|-------------|-------|
| Try | `x?` | `Result<T,E>` or `Option<T>` | `T` | Propagates `Err` / `None` via early `return`; hard error otherwise (E0114) |
| Optional access | `a?.field` | `Option<R>` with field `U` | `Option<U>` | Option-only; Result → E0116 |
| Coalesce | `a ?? b` | `Option<T>` or `Result<T,E>` | `T` | RHS must unify with `T`; **`??` on Result swallows `Err`** (document / prefer `?` when failure matters) |

Precedence sketch:

```
a?.x ?? b? ?? c   // (a?.x) ?? ((b?) ?? c)   — ?? is right-associative
a || b ?? c       // (a || b) ?? c
a = b ?? c        // a = (b ?? c)
```

`raise expr` is a keyword expression (not a Pratt operator); it produces `Err(expr)` and requires result mode.

---

## Indexing (`[]`)

| Form | Example | Precedence |
|------|---------|------------|
| Postfix index | `arr[i]`, `t[0]` | Primary |
| Empty index append | `arr[] = v` (LHS only) | Primary |

`arr[]` with no index expression is only valid as the target of `=` / `+=` — it appends to a dynamic array.

---

## Unary operators

| Operator | Name | Operand | Result |
|----------|------|---------|--------|
| `-` | Negate | numeric | numeric |
| `+` | Positive | numeric | numeric (no-op) |
| `~` | Bitwise NOT | `int` | `int` (flip bits) |
| `!` | Logical NOT | `bool` or `int` | `bool` |

For `!` on integers, zero is false and any non-zero value is true (`!0` → `true`, `!42` → `false`).

---

## Operator parsing notes

| Input | Parses as | Not as |
|-------|-----------|--------|
| `(1 + 2) * 3` | Group then multiply | |
| `(1, 2)` | Tuple | Two groups |
| `(1)` | Group | 1-tuple |
| `(1,)` | 1-tuple | |
| `1.0` | Float literal | `1` `.` `0` |
| `a++` | Postfix inc | |
| `!=` | Single operator | `!` `=` |
| `!true` | Prefix logical NOT | `!` applied to `true` |

---

## Related documents

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Full expression grammar |
| [Types](types.md) | Unification on operator operands |
| [Keywords](keywords.md) | `true`, `false`, `new`, etc. |
