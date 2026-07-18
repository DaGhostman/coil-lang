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
function_decl ::= 'async'? 'fn' IDENT type_param_list? arg_list
                  ('->' type_annotation)? where_clause? block
type_param_list ::= '<' type_param (',' type_param)* '>'
type_param      ::= IDENT (':' (kind | class_bound ('+' class_bound)*))?
kind            ::= '*' | 'Constraint' | kind '->' kind | '(' kind ')'
class_bound     ::= IDENT
where_clause    ::= 'where' where_constraint (',' where_constraint)*
where_constraint ::= IDENT '<' type_annotation (',' type_annotation)* '>'
arg_list      ::= '(' (type_annotation IDENT (',' type_annotation IDENT)*)? ')'
```

Examples:

```0s
fn add(int a, int b) -> int { return a + b; }
fn add<T: Num>(T a, T b) -> T { return a + b; }
fn apply_cast<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
fn greet() { print "hi"; }
```

### Traits and impl

```
trait_decl ::= 'trait' IDENT type_param_list '{' trait_item* '}'
trait_item ::= assoc_type_decl | method_sig
assoc_type_decl ::= 'type' IDENT type_param_list? ';'
method_sig     ::= 'fn' IDENT arg_list ('->' type_annotation)? (';' | block)
impl_decl      ::= 'impl' IDENT type_arg_list? 'for' type '{' impl_item* '}'
                 | 'impl' IDENT type_arg_list '{' impl_item* '}'   // legacy
impl_item      ::= assoc_type_def | method_decl
assoc_type_def ::= 'type' IDENT type_param_list? '=' type ';'
type_arg_list  ::= '<' type (',' type)* '>'
type_projection ::= IDENT '::' IDENT type_arg_list?
                 // e.g. Collect::Elem, C::Elem, Pointer::Ref<int>, P::Ref<A>
```

The type after `for` is prepended as the first type argument (Self slot):
`impl Show for Foo` ≡ `impl Show<Foo>`, and
`impl Thing<A, B> for Foo` ≡ `impl Thing<Foo, A, B>`.

Example:

```0s
// Builtin arithmetic: Add / Sub / Mul / Div (Num implies all four).
// Builtin ordering: Lt / Le / Gt / Ge (Ord implies all four).

trait Collect<C> {
    type Elem;
    fn head(C xs) -> Elem;
}

impl Collect for Option<int> {
    type Elem = int;
    fn head(Option<int> xs) -> int { /* … */ }
}

trait Pointer<P: * -> *> {
    type Ref<T>;
    fn deref<T>(P<T> ptr) -> Ref<T>;
}

impl Pointer for Option {
    type Ref<T> = T;
    fn deref<T>(Option<T> ptr) -> T { /* … */ }
}

impl Measurable for int {
    fn size(int x) -> int { return x; }
}

// Legacy angle-bracket form (still accepted):
impl Measurable<int> {
    fn size(int x) -> int { return x; }
}
```

Generic functions use `type_param_list` on `fn` (see above). Bounds use `+`
between trait names (`T: Num + Eq` or `T: Add`). Multi-parameter traits use a trailing
`where Trait<T1, T2>` clause (unary `where Num<T>` is also accepted).
Higher-kinded parameters use explicit kind annotations (`F: * -> *`,
`F: * -> * -> *`, or `F: (* -> *) -> *`); a bound whose trait parameter is
constructor-kinded (for example `F: Container`) also implies that kind. A
parameter can carry both an explicit kind and a bound:
`F: * -> * -> *, Bifunctor`.

Constraint-kind parameters use `Constraint` as the result kind:
`fn apply_c<c: * -> Constraint, T: c>(T x) -> string { return show(x); }`.
The abstract `T: c` bound must be resolved by method/operator/`%v` use in the
function body so codegen can pass a concrete dictionary at call sites.

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

Grammar (with optional derive):

```
enum_decl ::= 'enum' IDENT type_param_list? derive_clause? '{' variant (',' variant)* ','? '}'
derive_clause ::= 'derive' IDENT (',' IDENT)*
```

Examples:

```0s
enum Tree { Leaf, Node(int, Tree, Tree) }
enum Point { Origin, Point { x: int, y: int } }
enum Color derive Show, Eq, Ord { Red, Blue }
```

### Type aliases

```
type_alias ::= 'type' IDENT type_param_list? '=' type_annotation ';'
```

Examples: `type PointPair = (int, int);`, `type Pair<T> = (T, T);`

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
class_decl ::= 'class' IDENT type_param_list? derive_clause? '{' field_decl (',' field_decl)* ','? '}'
field_decl ::= 'pub'? IDENT ':' type
derive_clause ::= 'derive' IDENT (',' IDENT)*

impl_decl  ::= 'impl' IDENT type_param_list? '{' method_decl* '}'
method_decl ::= 'pub'? function_decl
```

`type_param_list` is the same form as on functions (`<T>`, `<T: Num>`, …).
An inherent `impl Cell<T>` shares those parameters with the class so methods
can mention `T` and type `self` as `Cell<T>`.
See [Trait derive](types.md#trait-derive) for the `derive` clause.

Example:

```0s
class Foo { pub name: string, count: int, }
impl Foo {
    pub fn bump() -> int { return 1; }
    fn name_len() -> int { return 0; }
}

class Cell derive Show, Eq { value: int }

class Cell<T> { value: T }
impl Cell<T> {
    fn get() -> T { return self.value; }
}
```

Classes support positional constructor args (field order), field read/write, and method calls with implicit `self`. See `examples/classes.0s` and `examples/generic_class.0s`.

Note: trait `impl` (`impl Collect<Option<int>> { … }`) uses a different
parse path — see [Traits and impl](#traits-and-impl) above.

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
            | for_stmt
            | break_stmt
            | continue_stmt
            | if_stmt
            | block
            | let_stmt
            | const_stmt
            | expr_stmt
            | print_stmt
            | return_stmt
            | comment
```

| Statement | Syntax |
|-----------|--------|
| `let` | `let IDENT (':' type_annotation)? ('=' expr)? ';'` |
| `const` | `const IDENT (':' type_annotation)? '=' expr ';'` |
| Expression | `expr ';'` |
| `print` | `print STRING (',' expr)* ';'` |
| `return` | `return expr ';'` |
| `yield` | `yield expr ';'` or `yield from expr ';'` |
| `while` | `while expr block` |
| `for` | `for '(' init ';' cond ';' step ')' block` (C-style; desugars to `while`) |
| `break` | `break ';'` (innermost loop) |
| `continue` | `continue ';'` (jumps to `for` step / `while` condition) |
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
       | resume_expr | yield_expr
       | tuple_lit | array_lit | dict_lit
       | construct | call | instantiate
       | float | int | string
       | 'true' | 'false'
       | 'new' IDENT ('(' args? ')')?
       | IDENT
       | group
```

`dload` / `declare` / `invoke` are ordinary `IDENT` calls after `use ffi::*` (not keyword atoms).

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
assignment ::= lvalue assign_op expr
assign_op    ::= '=' | '+=' | '-=' | '*=' | '/=' | '%=' | '**=' | '<<=' | '>>=' | '&=' | '|=' | '^='
lvalue       ::= IDENT | access | index
adjust       ::= ('++' | '--') lvalue | lvalue ('++' | '--')
```

Compound assignment is right-associative. Prefix/postfix `++`/`--` bind at unary and primary precedence respectively.

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
type_annotation ::= array_type | tuple_type | type_projection | IDENT
array_type      ::= '[' type (';' INT)? ']'
tuple_type      ::= '(' type (',' type)+ ')'
type_projection ::= IDENT '::' IDENT type_arg_list?
```

| Form | Meaning |
|------|---------|
| `int` | Primitive or type constructor name |
| `[int]` | Dynamic-length array |
| `[int; 5]` | Static-length array (length 5) |
| `(int, string)` | Tuple type (comma required) |
| `C::Elem` | Associated type projection |
| `P::Ref<A>` | Generic associated type projection with type arguments |

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

These appear in planning docs but are **not** parsed from source today:

| Feature | Status |
|---------|--------|
| `case` as alias for `match` | Not registered |
| Iterator `for x in …` | Not implemented — use C-style `for` or `while` |

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
