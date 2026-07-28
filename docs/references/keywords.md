# Keywords reference

All reserved words in the coil parser. Keywords cannot be used as identifiers.

---

## Keyword index

| Keyword | Category | Brief description | More info |
|---------|----------|-------------------|-----------|
| `fn` | Declaration | Function definition | [Syntax — Functions](syntax.md#functions) |
| `let` | Statement | Mutable local binding | [Syntax — Statements](syntax.md#statements) |
| `const` | Statement | Immutable local binding (shallow: heap interiors may still mutate) | [Syntax — Statements](syntax.md#statements) |
| `static` | Declaration | Module or class singleton slot | [Types — Statics](types.md#static-slots) |
| `readonly` | Expression | Seal value against external mutation | [Types — Readonly](types.md#readonly-types) |
| `if` | Statement | Conditional | [Syntax — Statements](syntax.md#statements) |
| `else` | Statement | Alternative branch | [Syntax — Statements](syntax.md#statements) |
| `while` | Statement | Loop while condition true | [Syntax — Statements](syntax.md#statements) |
| `for` | Statement | C-style `for (…)` or iterator `for x in expr` | [Syntax — Statements](syntax.md#statements) |
| `in` | Statement | For-in separator (`for x in expr` via `IntoIterator`) | [Built-ins — Iterator](iterator.md) |
| `break` | Statement | Exit innermost loop | [Syntax — Statements](syntax.md#statements) |
| `continue` | Statement | Next iteration of innermost loop | [Syntax — Statements](syntax.md#statements) |
| `return` | Statement | Exit function with value | [Syntax — Statements](syntax.md#statements) |
| `raise` | Expression / stmt | Early-return `Err(e)` (result mode) | [Tutorial: Error handling](../manual/tutorial/09-error-handling.md) |
| `panic` | Expression / stmt | Abort with a string message | [Built-ins](panic.md) |
| `print` | Statement / I/O | Print to stdout | [Built-ins](print.md) |
| `format` | Expression / I/O | Build a formatted string | [Built-ins](format.md) |
| `enum` | Declaration | Sum type definition | [Types — Sum types](types.md#sum-types--enums-tysum) |
| `match` | Expression | Pattern match | [Syntax — Patterns](syntax.md#patterns-match) |
| `default` | Pattern | Wildcard arm (same as `_`) | [Syntax — Patterns](syntax.md#patterns-match) |
| `type` | Declaration | Type alias | [Types — Aliases](types.md#type-aliases-type-name--t) |
| `use` | Declaration | Import module item | [Modules](modules.md) |
| `as` | Import | Rename imported item | [Modules](modules.md#aliasing-rules) |
| `mod` | Declaration | Forward-declare / load module | [Modules](modules.md) |
| `extern` | Declaration | FFI library block | [FFI tutorial](../manual/tutorial/07-ffi.md) |
| `class` | Declaration | Class with fields | [Syntax — Classes](syntax.md#classes-and-impl) |
| `impl` | Declaration | Class methods or trait instances (`impl Trait for T`) | [Syntax — Classes](syntax.md#classes-and-impl) / [Types — Traits](types.md#generics-and-traits) |
| `pub` | Modifier | Public field or method | [Syntax — Classes](syntax.md#classes-and-impl) |
| `new` | Expression | Construct class instance | [Syntax — Expressions](syntax.md#atoms-primary-forms) |
| `defer` | Declaration | Run block on function exit (`defer use (x) { … }` captures outer locals) | [Syntax — Defer](syntax.md#defer) |
| `true` | Literal | Boolean true | [Types — Primitives](types.md#primitive-types) |
| `false` | Literal | Boolean false | [Types — Primitives](types.md#primitive-types) |
| `dload` / `declare` / `invoke` | Ordinary names | FFI callables from virtual `ffi` (not keywords) | [Built-ins — FFI](ffi.md) |
| `async` | Declaration | Coroutine function (`coroutine<Y>` / `coroutine<Y, S>`) | [Tutorial: Coroutines](../manual/tutorial/08-coroutines.md) |
| `yield` | Expression / stmt | Suspend coroutine; optional receive binding | [Tutorial: Coroutines](../manual/tutorial/08-coroutines.md) |
| `yield from` | Expression / stmt | Delegate to sub-coroutine | [Tutorial: Coroutines](../manual/tutorial/08-coroutines.md) |
| `resume` | Expression | Continue coroutine handle | [Tutorial: Coroutines](../manual/tutorial/08-coroutines.md) |
| `with` | Resume modifier | Send value on resume (`resume h with v`) | [Tutorial: Coroutines](../manual/tutorial/08-coroutines.md) |
| `where` | Declaration | Constraint clause on generic functions | [Types — Generics](types.md#generics-and-traits) |
| `trait` | Declaration | User-defined trait | [Types — Generics](types.md#generics-and-traits) |

---

## Declaration keywords

```
fn | enum | type | trait | use | mod | extern | class | impl | defer | async | where | attr
```

Attributes (`#[derive(...)]`, `#[test]`, `#[ffi(...)]`, user `#[name(...)]`) are not keywords — see [Syntax — Attributes](syntax.md#attributes).

Registered in the top-level `declaration()` parser before generic statements so keywords like `enum` are not misparsed as `let`.

---

## Statement keywords

```
let | const | if | else | while | for | break | continue | return | raise | panic | print
```

Appear inside `{ ... }` blocks via `statement()`.

---

## Expression / literal keywords

```
match | new | true | false | format | yield | resume | done | raise | panic
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
| `case` | Planned alias for `match`; not registered |
| `import` | Not implemented — use `use` |
| `struct` | FFI `extern struct` only; otherwise use `class` or record dicts |

---

## Keywords vs builtins

| Kind | Examples | Callable as `name(...)`? |
|------|----------|------------------------|
| Statement keyword | `print` | No — statement form only: `print "...";` |
| Virtual-module export | `dload`, `declare`, `invoke` (via `use ffi::*`) | Yes as identifiers — not reserved keywords |
| Declaration keyword | `fn`, `enum` | No |

---

## Related documents

| Document | Contents |
|----------|----------|
| [Syntax](syntax.md) | Grammar using these keywords |
| [Built-ins](README.md) | `print`, FFI functions |
| [Operators](operators.md) | Non-keyword operators |
