# Syntax reference

Complete grammar overview for zero-script source (`.0s`). This document describes what the parser accepts today; see [Types](types.md), [Operators](operators.md), and [Keywords](keywords.md) for semantics.

---

## File formats

| Extension | Role |
|-----------|------|
| `.0s` | Source text parsed by `parser::Pratt` |
| `.c0s` | Compiled bytecode archive (rkyv-serialized `ArchivedProgram`) |

Programs are sequences of **declarations** and **statements**. Most top-level items are declarations; statements appear inside function bodies and blocks.

---

## Lexical structure

| Element | Rules |
|---------|-------|
| Identifiers | ASCII letters, digits, underscore; must not be a [keyword](keywords.md) |
| Integers | Decimal (`42`, `-1`) |
| Floats | Decimal with fraction (`1.0`, `3.14`) — parsed before postfix `.field` |
| Strings | `"..."` — double-quoted, no escape sequences beyond `\"` not supported (no `\n` escapes in lexer) |
| Comments | `//` to end of line |
| Whitespace | Insignificant except as token separator |

---

## Program

```
program ::= declaration*
```

Every runnable program needs `fn main() { ... }` (or an entry file declared in `zero.toml`).

---

## Declarations

Top-level forms (order in parser `choice`):

```
declaration ::= class_decl
              | impl_decl
              | function_decl
              | type_alias
              | use_stmt
              | mod_stmt
              | enum_decl
              | defer_stmt          // at declaration level in parser chain
              | extern_block
              | statement
```

### Functions

```
function_decl ::= 'async'? 'fn' IDENT arg_list ('->' type_annotation)? block
arg_list      ::= '(' (type_annotation IDENT (',' type_annotation IDENT)*)? ')'
```

Examples:

```0s
fn add(int a, int b) -> int { return a + b; }
fn greet() { print "hi"; }
```

### Enums

```
enum_decl   ::= 'enum' IDENT '{' enum_variant (',' enum_variant)* ','? '}'
enum_variant ::= IDENT variant_payload?
variant_payload ::= unit | tuple_payload | record_payload
unit            ::= /* nothing, or empty () */
tuple_payload   ::= '(' type (',' type)* ')'
record_payload  ::= '{' field_decl (',' field_decl)* '}'
field_decl      ::= IDENT ':' type
```

Examples:

```0s
enum Option { None, Some(int) }
enum Point { Origin, Point { x: int, y: int } }
enum Tree { Leaf, Node(int, Tree, Tree) }
```

### Type aliases

```
type_alias ::= 'type' IDENT '=' type_annotation ';'
```

Example: `type PointPair = (int, int);`

### Modules

```
use_stmt ::= 'use' path ('as' IDENT)? ';'
path     ::= IDENT ('::' IDENT)* ('::' '*')?
mod_stmt ::= 'mod' IDENT ';'
```

See [Modules reference](modules.md).

### Extern (FFI)

```
extern_block ::= 'extern' STRING '{' extern_fn* '}'
extern_fn    ::= 'fn' IDENT arg_list ('->' IDENT)? ';'
```

Example:

```0s
extern "libc.so.6" {
    fn strlen(string s) -> int;
}
```

See [FFI tutorial](../tutorial/07-ffi.md).

### Classes and impl

```
class_decl ::= 'class' IDENT '{' field_decl (',' field_decl)* ','? '}'
field_decl ::= 'pub'? IDENT ':' type

impl_decl  ::= 'impl' IDENT '{' method_decl (',' method_decl)* ','? '}'
method_decl ::= 'pub'? function_decl
```

Example:

```0s
class Foo { pub name: string, count: int }
impl Foo {
    pub fn bump() -> int { return 1; }
}
```

Classes are partially supported at runtime — see [Getting Started](../getting-started.md).

### Defer

```
defer_stmt ::= 'defer' block
```

Runs when the enclosing function exits (LIFO order for multiple defers).

---

## Statements

Inside `{ ... }` blocks:

```
statement ::= while_stmt
            | if_stmt
            | block
            | let_stmt
            | expr_stmt
            | print_stmt
            | return_stmt
            | comment
```

| Statement | Syntax |
|-----------|--------|
| `let` | `let IDENT (':' type_annotation)? ('=' expr)? ';'` |
| Expression | `expr ';'` |
| `print` | `print STRING (',' expr)* ';'` |
| `return` | `return expr ';'` |
| `yield` | `yield expr ';'` or `yield from expr ';'` |
| `while` | `while expr block` |
| `if` | `if expr block ('else' (block \| if_stmt))?` |
| Block | `'{' statement* '}'` |

### `let` desugaring

`let x: int = 5;` produces a variable declaration fragment followed by initializer expression. Type-only `let x: int;` is allowed.

---

## Expressions

Expression grammar uses a **Pratt parser** with atoms and operator precedence (see [Operators](operators.md)).

### Atoms (primary forms)

```
atom ::= match_expr
       | dload_call | declare_call | invoke_call
       | resume_expr | yield_expr
       | tuple_lit | array_lit | dict_lit
       | construct | call | instantiate
       | float | int | string
       | 'true' | 'false'
       | 'new' IDENT ('(' args? ')')?
       | IDENT
       | group
```

| Form | Syntax | Notes |
|------|--------|-------|
| Group | `(expr)` | Single expr — **not** a 1-tuple |
| Tuple | `(e1, e2)` or `(e,)` | Comma required for tuple |
| Array | `[e1, e2, ...]` or `[]` | Homogeneous elements |
| Dict | `{ name: expr, ... }` | Anonymous record |
| Construct | `Enum::Variant(...)` | Qualified constructor |
| Call | `f(args)` | Includes user functions and FFI-wrapped extern fns |
| Instantiate | `new Class(args)` | Class construction |
| Match | `match expr '{' arm (',' arm)* '}'` | See patterns below |
| Index | `expr '[' expr ']'` | Postfix |
| Access | `expr '.' IDENT` | Postfix field access |
| Resume | `resume expr ('with' expr)?` | Continue coroutine; optional send value |
| Yield | `yield expr` | Suspend with yielded value |
| Yield from | `yield from expr` | Delegate to sub-coroutine handle |

### Coroutines

```
async_fn     ::= 'async' function_decl
resume_expr  ::= 'resume' expr ('with' expr)?
yield_expr   ::= 'yield' ('from' expr | expr)
binding_yield ::= 'let' IDENT '=' yield_expr
```

Examples:

```0s
async fn ping() {
    let msg = yield "ready";
    print "%s", msg;
}

fn main() {
    let h = ping();
    resume h;
    resume h with "hello";
}
```

See [Tutorial: Coroutines](../tutorial/08-coroutines.md).

### Assignment

Assignment is an expression (lowest precedence):

```
assignment ::= lvalue '=' expr
lvalue     ::= IDENT | access | index  /* field/index LHS for dict mutation */
```

---

## Patterns (`match`)

```
pattern ::= '_' | 'default'
          | IDENT
          | IDENT '::' IDENT pattern_payload?
pattern_payload ::= unit | tuple_pattern | record_pattern
tuple_pattern   ::= '(' pattern (',' pattern)* ')'
record_pattern  ::= '{' field_pattern (',' field_pattern)* '}'
field_pattern   ::= IDENT (':' pattern)?   /* shorthand: x => x: x */
```

Examples:

```0s
match x {
    Option::None => 0,
    Option::Some(v) => v,
    _ => -1,
}

match p {
    Point::Point { x, y } => x + y,
}
```

---

## Type annotations

Used in function signatures, `let`, enum payloads, and type aliases:

```
type_annotation ::= array_type | tuple_type | IDENT
array_type      ::= '[' type (';' INT)? ']'
tuple_type      ::= '(' type (',' type)+ ')'
```

| Form | Meaning |
|------|---------|
| `int` | Primitive or type constructor name |
| `[int]` | Dynamic-length array |
| `[int; 5]` | Static-length array (length 5) |
| `(int, string)` | Tuple type (comma required) |

Primitive names are case-insensitive in the typechecker (`String` ≡ `string`).

---

## `match` arms

```
arm ::= pattern '=>' expr
```

Arms are comma-separated inside `match { ... }`. The last arm may use `_` or `default` as wildcard.

---

## Entry point and compilation

| Rule | Detail |
|------|--------|
| Entry function | `fn main()` required for standalone programs |
| Prologue | Compiler emits `CALL`, `JMP`, `HALT`; patches jump to `main` |
| Extern setup | `extern` blocks may emit setup before `main` |
| Archive | Output wrapped in `ArchivedProgram { version, bytecode, ... }` |

---

## Multi-file projects

With `zero.toml`, the pipeline discovers dependencies via `use` / `mod` and compiles each file with a namespace prefix. The **entry file** uses the empty namespace. See [Modules reference](modules.md).

---

## Not yet in the grammar

These appear in planning docs or internal AST nodes but are **not** parsed from source today:

| Feature | Status |
|---------|--------|
| `const` declarations | AST support only; use `let` |
| `format` as keyword | Use `print "%i", x` |
| `async` / `yield` / `resume` | Coroutine opcodes exist; parser keywords not wired |
| `case` as alias for `match` | Not registered |
| String concat `+` | Not in current typechecker/codegen tree |

See [README](../README.md) language-at-a-glance table for the live feature matrix.

---

## Related documents

| Document | Contents |
|----------|----------|
| [Types](types.md) | Type forms and inference |
| [Operators](operators.md) | Precedence and semantics |
| [Keywords](keywords.md) | Reserved words |
| [Built-ins](built-ins.md) | `print`, FFI builtins |
| [Modules](modules.md) | `use` / `mod` resolution |
