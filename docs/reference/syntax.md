# Syntax reference

Complete grammar overview for coil source (`.hy`). This document describes what the parser accepts today; see [Types](types.md), [Operators](operators.md), and [Keywords](keywords.md) for semantics.

---

## File formats

| Extension | Role |
|-----------|------|
| `.hy` | Source text parsed by `parser::Pratt` |
| `.hyc` | Compiled bytecode archive (rkyv-serialized `ArchivedProgram`) |

Programs are sequences of **declarations** and **statements**. Most top-level items are declarations; statements appear inside function bodies and blocks.

---

## Attributes

Rust-style attributes attach metadata to top-level `fn`, `enum`, and `class` declarations (and to methods inside `impl` blocks):

```
attr_list   ::= ('#[' attribute ']')*
attribute   ::= ident
              | ident '(' ident (',' ident)* ')'
              | ident '(' kv (',' kv)* ')'
              | ident '(' literal (',' (literal | kv))* ')'
              | ident '(' string ')'
kv          ::= ident '=' (string | int | float | 'true' | 'false')
literal     ::= string | int | float | 'true' | 'false'
```

Examples:

```coil
#[derive(Show, Eq, Ord)]
enum Color { Red, Blue }

#[test]
fn add_works() {
    assert(1 + 1 == 2)?;
}

#[ffi(lib = "c", name = "strlen")]
fn strlen(string s) -> int;

attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    print "%s", message;
    return target(...args);
}

#[log(message = "enter")]
fn do_work(int x) -> int { return x; }

#[log(message = "Point ctor")]
class Point { x: int, y: int }
```

| Attribute | Target | Semantics |
|-----------|--------|-----------|
| `#[derive(Trait, …)]` | `enum` / `class` | Synthesizes builtin trait instances (`Show`, `Eq`, `Ord`) |
| `#[test]` / `#[test("desc")]` | `fn` with body | Registers a `coil test` harness case (Result mode) |
| `#[ffi(lib = "…", name = "…", variadic = true)]` | signature-only `fn …;` | Desugars to compile-time `extern` lowering |
| User `attr` names | `fn`, methods, `class` | Expands to a wrapper that receives the decoratee callable, attribute extras, and forwarded call arguments (`...args`); class attrs wrap the constructor |

User-defined attributes must end with a bare tuple-rest parameter `...args` and call `target(...args)` to forward runtime arguments. Stacking order is Python-style: the first listed attribute is outermost. User attrs cannot be applied to FFI bindings.

Multiple attributes stack (e.g. `#[derive(Show)] #[derive(Eq)]`). Unknown attribute names are rejected at compile time.

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

Every runnable program needs `fn main() { ... }` (or an entry file declared in `coil.toml`).

---

## Declarations

Top-level forms (order in parser `choice`):

```
declaration ::= class_decl
              | impl_decl
              | function_decl
              | attr_decl
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
function_decl ::= attr_list? 'async'? 'fn' IDENT type_param_list? arg_list
                  ('->' type_annotation)? where_clause? (block | ';')
type_param_list ::= '<' type_param (',' type_param)* '>'
type_param      ::= IDENT (':' (kind | class_bound ('+' class_bound)*))?
kind            ::= '*' | 'Constraint' | kind '->' kind | '(' kind ')'
class_bound     ::= IDENT
where_clause    ::= 'where' where_constraint (',' where_constraint)*
where_constraint ::= IDENT '<' type_annotation (',' type_annotation)* '>'
arg_list      ::= '(' (arg (',' arg)*)? ')'
arg           ::= type_annotation '...'? IDENT   // `T... name` → `[T]`; bare `... name` → tuple pack
```

### Attribute declarations

```
attr_decl   ::= 'attr' IDENT type_param_list? '(' attr_param (',' attr_param)* ')'
                ('->' type_annotation)? where_clause? block
attr_param  ::= type_fn_sig IDENT          // first param: `fn(...args) -> T target`
              | type_annotation IDENT      // middle extras: `string message`, …
              | '...' IDENT                // trailing tuple-rest: forwarded call args
type_fn_sig ::= 'fn' '(' '...' IDENT ')' '->' type_annotation
```

Arity overloads (same name, different arities / rest ranges), first-class monomorphic functions (`let f = add`), positional and named partial application, and explicit-capture lambdas (`fn (T x) use (y) => …`) are supported. See `examples/overload.hy`, `fn_value.hy`, and `lambda.hy`.


Call sites may use named arguments (`name: expr`) after any positional
prefix. Rest parameters are positional-only and pack trailing values
into one array (`T... xs` → `[T]`) or tuple (`... xs` → `(T1, …, Tn)`).
Call-site spread forwards packed values as separate arguments:

```
call        ::= callee '(' (call_arg (',' call_arg)*)? ')'
call_arg    ::= expr | '...' expr
```

| Spread operand type | Semantics |
|---------------------|-----------|
| Tuple `(T1, …, Tn)` | Each element becomes one argument |
| Fixed array `[T; N]` | Each element becomes one argument |
| Dynamic `[T]` | Compile error (use tuple or `[T; N]`) |
| Other | Compile error |

Examples:

```coil
fn triple(int a, int b, int c) -> int { return a + b + c; }
triple(...(1, 2, 3));          // tuple spread
triple(...[10, 20, 30]);       // array spread

attr wrap<T>(fn(...args) -> T target, ...args) -> T {
    return target(...args);
}
```

```coil
fn add(int a, int b) -> int { return a + b; }
fn add<T: Num>(T a, T b) -> T { return a + b; }
fn apply_cast<A, B>(A x) -> B where Convert<A, B> { return cast(x); }
fn greet() { print "hi"; }
fn sum(int... xs) -> int { return len(xs); }
sum(1, 2, 3);   // xs == [1, 2, 3]
sum();          // xs == []

// Signature-only FFI declaration (requires #[ffi(...)]):
#[ffi(lib = "c")]
fn strlen(string s) -> int;
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

```coil
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

Grammar (with optional derive attribute):

```
enum_decl ::= attr_list? 'enum' IDENT type_param_list? '{' variant (',' variant)* ','? '}'
```

Example:

```coil
enum Tree { Leaf, Node(int, Tree, Tree) }
enum Point { Origin, Point { x: int, y: int } }
#[derive(Show, Eq, Ord)]
enum Color { Red, Blue }
```

### Type aliases

```
type_alias ::= 'type' IDENT type_param_list? '=' type_annotation ';'
```

Examples: `type PointPair = (int, int);`, `type Pair<T> = (T, T);`

### Modules

```
use_stmt ::= 'use' use_path ';'
use_path ::= IDENT ('::' IDENT)* '::' '{' use_item (',' use_item)* ','? '}'
           | IDENT ('::' IDENT)* ('::' '*')?
           | IDENT ('::' IDENT)* ('as' IDENT)?
use_item ::= IDENT ('as' IDENT)?
mod_stmt ::= 'mod' IDENT ';'
```

Brace groups (`use math::{add, mul};`) desugar to multiple single-item `use`s.
See [Modules reference](modules.md).

### Extern (FFI)

```
extern_block    ::= 'extern' STRING '{' extern_fn* '}'
extern_fn       ::= 'fn' IDENT extern_arg_list ('->' type)? ';'
extern_arg_list ::= '(' (T name (',' T name)* (',' '...')? | '...')? ')'
```

Fixed parameters use `T name` (same as ordinary FFI args). A trailing bare
`...` marks C-style varargs (`printf`-style). Language rest `T... name` is
rejected inside `extern` — use bare `...` instead.

Example:

```coil
extern "c" {
    fn strlen(string s) -> int;
    fn printf(string fmt, ...) -> int;
}
```

`extern "c"` is the portable libc alias. See [FFI tutorial](../tutorial/07-ffi.md).

Equivalent attribute form for a single function:

```coil
#[ffi(lib = "c")]
fn strlen(string s) -> int;
```

### Classes and impl

```
class_decl ::= attr_list? 'class' IDENT type_param_list? '{' field_decl (',' field_decl)* ','? '}'
field_decl ::= 'pub'? IDENT ':' type

impl_decl  ::= 'impl' IDENT type_param_list? '{' method_decl* '}'
method_decl ::= 'pub'? function_decl
```

`type_param_list` is the same form as on functions (`<T>`, `<T: Num>`, …).
An inherent `impl Cell<T>` shares those parameters with the class so methods
can mention `T` and type `self` as `Cell<T>`.
See [Trait derive](types.md#trait-derive) for `#[derive(...)]`.

Example:

```coil
class Foo { pub name: string, count: int, }
impl Foo {
    pub fn bump() -> int { return 1; }
    fn name_len() -> int { return 0; }
}

#[derive(Show, Eq)]
class Cell { value: int }

class Cell<T> { value: T }
impl Cell<T> {
    fn get() -> T { return self.value; }
}
```

Classes support positional constructor args (field order), field read/write, and method calls with implicit `self`. See `examples/classes.hy` and `examples/generic_class.hy`.

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
| `static` | `static let IDENT …` / `static const IDENT …` (top-level only) |
| Expression | `expr ';'` |
| `print` | `print STRING (',' expr)* ';'` |
| `return` | `return expr ';'` |
| `yield` | `yield expr ';'` or `yield from expr ';'` |
| `while` | `while expr block` |
| `for` (C-style) | `for '(' init ';' cond ';' step ')' block` |
| `for` (iterator) | `for IDENT in expr block` — via prelude `IntoIterator` / `Iterator` (arrays, homogeneous tuples/dicts, coroutines, or user `impl`s; see [Built-ins](built-ins.md#iterator--intoiterator)) |
| `break` | `break ';'` (innermost loop) |
| `continue` | `continue ';'` (jumps to `for` step / `while` condition / next for-in iteration) |
| `if` | `if expr block ('else' (block \| if_stmt))?` |
| Block | `'{' statement* '}'` |

### `let` desugaring

`let x: int = 5;` produces a variable declaration fragment followed by initializer expression. Type-only `let x: int;` is allowed.

`static let` / `static const` are top-level declarations only; initializers run in the program prologue before `main`.

### Array append (`arr[] =`)

Empty index `arr[]` is valid **only** as an assignment target and appends to a dynamic array:

```coil
arr[] = value;
```

Using `arr[]` as an rvalue is a compile error. See [Built-ins — Array append](built-ins.md#array-append-arr--and-len).

### `readonly` expressions

Prefix `readonly` on array literals or `new` seals the value against external mutation:

```coil
let xs = readonly [1, 2, 3];
let p = readonly new Point(1, 2);
```

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
| Call | `f(args)` | Args are positional `expr` and/or named `name: expr` (positional prefix, then named; no positional after named). Includes user functions and FFI-wrapped extern fns |
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

```coil
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

```coil
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
arm ::= pattern '=>' (block_expr | expr)
block_expr ::= '{' (expr ';'?)* '}'
```

Arms are comma-separated inside `match { ... }`. The last arm may use `_` or `default` as wildcard.

Brace bodies (`{ … }`) are **expression blocks**, not dict literals — so
`self.method()` and other non-`name: value` forms work inside them.
A dict arm still works when the body is a real record literal (`{ x: 1 }`).

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

With `coil.toml`, the pipeline discovers dependencies via `use` / `mod` and compiles each file with a namespace prefix. The **entry file** uses the empty namespace. See [Modules reference](modules.md).

---

## Not yet in the grammar

These appear in planning docs but are **not** parsed from source today:

| Feature | Status |
|---------|--------|
| `case` as alias for `match` | Not registered |

### Ranges (lazy)

Half-open `start..end` and closed `start..=end` produce
**`Range<T: Ord>`** / **`RangeInclusive<T: Ord>`** (both bounds unify).
A range is a **lazy** iterable: `for x in 0..n` pulls one value at a
time and does **not** allocate an array of length `n`. Decreasing
ranges (`10..0`) are empty (Rust-like). First-class values work
(`let r = 0..n; for x in r`).

Construction needs only `Ord`. **`for` iteration** steps with `+1` /
`+1.0` for `int`, `byte`, and `float` (other `Ord` types may form a
range value but are not iterable yet).

```coil
for x in 0..5 { print "%i", x; }   // 01234
let r = 0..=3;
for x in r { print "%i", x; }      // 0123
for x in 1.0..4.0 { print "%f", x; } // 1.02.03.0
```

See `examples/range.hy`.

**Deferred:** turning a range into a concrete array (e.g. a future
`collect(0..5)` → `[0, 1, 2, 3, 4]`), step syntax, iterating non-numeric
`Ord` types.

Full design lock-in: [`FEATURE_PLAN_PARAMS_DESTRUCTURE_RANGES.md`](../../FEATURE_PLAN_PARAMS_DESTRUCTURE_RANGES.md).

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
