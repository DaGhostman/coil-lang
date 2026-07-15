# Operators reference

zero-script expressions use a **Pratt parser** with prefix, infix, and postfix operators. Higher rows in the [precedence table](#precedence-table-high-to-low) bind tighter.

Associativity:

| Class | Operators | Associativity |
|-------|-----------|---------------|
| Additive `+` `-` | Term level | **Left** |
| Most other binary | | **Right** |
| Assignment `=` | | **Right** |
| Postfix `++` `--` `.` `[]` | | N/A (postfix) |
| Prefix `-` `+` `~` | | N/A (prefix) |

---

## Precedence table (high to low)

| Precedence | Operators / forms | Notes |
|------------|-------------------|-------|
| **Primary (postfix)** | `expr++`, `expr--`, `expr.field`, `expr[index]` | Tightest — postfix on atoms |
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
| **Assignment** | `=` | Lowest — `lvalue = expr` |

Forms **not** in the Pratt table but still tight-binding:

| Form | Binding |
|------|---------|
| Function call `f(x)` | Atom — binds to identifier immediately |
| Qualified construct `E::V(...)` | Atom |
| Grouping `(expr)` | Atom |
| `match`, `new`, literals | Atoms |

---

## Arithmetic

| Operator | Types | Result | VM op (int / float) |
|----------|-------|--------|---------------------|
| `+` | `int` / `float` (both same) | same | `ADD` / `ADDF` |
| `-` | `int` / `float` | same | `SUB` / `SUBF` |
| `*` | `int` / `float` | same | `MUL` / `MULF` |
| `/` | `int` / `float` | same | `DIV` / `DIVF` |
| `%` | `int` / `float` | same | `MOD` / `MODF` |
| `**` | `int` / `float` | same | Power |

Mixed `int` and `float` operands → **type error** at compile time.

String concatenation with `+` is **not** implemented in the current language tree.

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

Short-circuit behavior follows VM evaluation order (both operands evaluated in current codegen).

---

## Comparison

| Operator | Operands | Result |
|----------|----------|--------|
| `==`, `!=` | Same type | `bool` |
| `<`, `<=`, `>`, `>=` | Same type (`int` or `float`) | `bool` |

Float and int comparisons use separate opcode families (`LE` vs `LEF`, etc.) selected at codegen from inferred types.

---

## Assignment

```
identifier = expr
field_access = expr    // dict record mutation (limited runtime)
```

| Rule | Detail |
|------|--------|
| LHS | Identifier, or field access on dict |
| Type | RHS must unify with existing binding |
| Undeclared | Error — use `let` first |
| Value | Assignment expression type is the RHS type |

`let` bindings use `StorePop`; match arms use no-op `STORE` for pattern slots.

---

## Postfix increment / decrement

| Form | Syntax | Notes |
|------|--------|-------|
| Postfix increment | `expr++` | Parsed at Primary precedence |
| Postfix decrement | `expr--` | Same |

Prefix increment on identifiers exists in parser helpers but is not exposed in the main expression Pratt chain.

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

## Indexing (`[]`)

| Form | Example | Precedence |
|------|---------|------------|
| Postfix index | `arr[i]`, `t[0]` | Primary |

Index expression is full `expr` — `arr[i + 1]` is valid.

---

## Unary operators

| Operator | Name | Operand | Result |
|----------|------|---------|--------|
| `-` | Negate | numeric | numeric |
| `+` | Positive | numeric | numeric (no-op) |
| `~` | NOT | `int` | `int` (bitwise) |

There is **no** postfix or prefix `!` for logical NOT — use `~` for bitwise negation on integers or compare to zero for booleans.

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

---

## Related documents

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Full expression grammar |
| [Types](types.md) | Unification on operator operands |
| [Keywords](keywords.md) | `true`, `false`, `new`, etc. |
