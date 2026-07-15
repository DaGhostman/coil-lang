# Keywords reference

All reserved words in the zero-script parser. Keywords cannot be used as identifiers.

---

## Keyword index

| Keyword | Category | Brief description | More info |
|---------|----------|-------------------|-----------|
| `fn` | Declaration | Function definition | [Syntax — Functions](syntax.md#functions) |
| `let` | Statement | Mutable local binding | [Syntax — Statements](syntax.md#statements) |
| `if` | Statement | Conditional | [Syntax — Statements](syntax.md#statements) |
| `else` | Statement | Alternative branch | [Syntax — Statements](syntax.md#statements) |
| `while` | Statement | Loop while condition true | [Syntax — Statements](syntax.md#statements) |
| `return` | Statement | Exit function with value | [Syntax — Statements](syntax.md#statements) |
| `print` | Statement / I/O | Print to stdout | [Built-ins](built-ins.md#print) |
| `enum` | Declaration | Sum type definition | [Types — Sum types](types.md#sum-types--enums-tysum) |
| `match` | Expression | Pattern match | [Syntax — Patterns](syntax.md#patterns-match) |
| `default` | Pattern | Wildcard arm (same as `_`) | [Syntax — Patterns](syntax.md#patterns-match) |
| `type` | Declaration | Type alias | [Types — Aliases](types.md#type-aliases-type-name--t) |
| `use` | Declaration | Import module item | [Modules](modules.md) |
| `as` | Import | Rename imported item | [Modules](modules.md#aliasing-rules) |
| `mod` | Declaration | Forward-declare / load module | [Modules](modules.md) |
| `extern` | Declaration | FFI library block | [FFI tutorial](../tutorial/07-ffi.md) |
| `class` | Declaration | Class with fields | [Syntax — Classes](syntax.md#classes-and-impl) |
| `impl` | Declaration | Methods for a class | [Syntax — Classes](syntax.md#classes-and-impl) |
| `pub` | Modifier | Public field or method | [Syntax — Classes](syntax.md#classes-and-impl) |
| `new` | Expression | Construct class instance | [Syntax — Expressions](syntax.md#atoms-primary-forms) |
| `defer` | Declaration | Run block on function exit | [Syntax — Defer](syntax.md#defer) |
| `true` | Literal | Boolean true | [Types — Primitives](types.md#primitive-types) |
| `false` | Literal | Boolean false | [Types — Primitives](types.md#primitive-types) |
| `dload` | Builtin | Load shared library | [Built-ins — FFI](built-ins.md#dload) |
| `declare` | Builtin | Register FFI signature | [Built-ins — FFI](built-ins.md#declare) |
| `invoke` | Builtin | Call FFI function | [Built-ins — FFI](built-ins.md#invoke) |

---

## Declaration keywords

```
fn | enum | type | use | mod | extern | class | impl | defer
```

Registered in the top-level `declaration()` parser before generic statements so keywords like `enum` are not misparsed as `let`.

---

## Statement keywords

```
let | if | else | while | return | print
```

Appear inside `{ ... }` blocks via `statement()`.

---

## Expression / literal keywords

```
match | new | true | false | dload | declare | invoke
```

Parsed as **atoms** before the generic `ident()` rule so they are never treated as variable names.

---

## Pattern keywords

```
default
```

Maps to `Pattern::Wildcard` — equivalent to `_` in match arms.

---

## Modifier keywords

```
pub
```

Optional prefix on class fields and `impl` methods. Default visibility is private.

---

## Reserved words (not keywords today)

These tokens are **not** in the parser keyword set. Using them as identifiers may work today but is discouraged — they may become keywords:

| Word | Notes |
|------|-------|
| `const` | AST node exists; no `const` keyword in parser — use `let` |
| `format` | Internal AST for format-string codegen; no user-facing keyword |
| `async` | Coroutine opcodes in VM; parser not wired |
| `yield` | Same |
| `resume` | Same |
| `case` | Planned alias for `match`; not registered |
| `break` | Not implemented |
| `continue` | Not implemented |
| `for` | Not implemented — use `while` |
| `import` | Not implemented — use `use` |
| `struct` | Not implemented — use `class` or record dicts |
| `trait` | Not implemented |
| `where` | Not implemented |
| `in` | Not implemented |

---

## Keywords vs builtins

| Kind | Examples | Callable as `name(...)`? |
|------|----------|------------------------|
| Statement keyword | `print` | No — statement form only: `print "...";` |
| Expression builtin | `dload`, `declare`, `invoke` | Yes — `dload("lib.so")` |
| Declaration keyword | `fn`, `enum` | No |

---

## Related documents

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Grammar using these keywords |
| [Built-ins](built-ins.md) | `print`, FFI functions |
| [Operators](operators.md) | Non-keyword operators |
