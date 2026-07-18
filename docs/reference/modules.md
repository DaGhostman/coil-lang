# Modules reference

This document specifies the syntax and semantics of zero-script's module system: `use` imports, `mod` forward declarations, namespace rules, and path resolution.

---

## Syntax

### `use` statement

```
use_stmt ::= 'use' path ('as' IDENT)? ';'
path     ::= IDENT ('::' IDENT)* ('::' '*')?
```

Forms:

| Form | Example |
|------|---------|
| Concrete import | `use foo::sadge;` |
| Aliased import | `use foo::sadge as f;` |
| Glob import | `use foo::*;` |
| Multi-segment | `use lib::io::read;` |

Rules:

- Every `use` statement ends with `;`.
- Path segments are identifiers separated by `::`.
- The last segment is either an identifier (concrete import) or `*` (glob).
- Only concrete imports may use `as`. Glob imports cannot be aliased.
- A glob marker (`*`) must be the **last** segment.

### `mod` statement

```
mod_stmt ::= 'mod' IDENT ';'
```

Example: `mod foo;`

A `mod` declaration loads a module file but does not bind any names in the current scope.

---

## Virtual modules (compiler builtins)

Some `use` paths resolve to **compiler-owned virtual modules**, not `.0s` files on disk. The pipeline skips disk discovery for these paths.

| Module | Exports | Auto-imported? |
|--------|---------|----------------|
| `prelude` | `Option`, `Result` | Yes (every file) |
| `prelude::ops` | `Add`, `Sub`, `Mul`, `Div`, `Num`, `Eq`, `Ord`, `Lt`, `Le`, `Gt`, `Ge`, `Show` | Yes (every file) |
| `prelude::test` | `assert` | Yes (every file) |
| `ffi` | `dload`, `declare`, `invoke` | No — write `use ffi::*;` |
| `ffi::types` | `Int`, `Float`, `String`, `Void`, `Ptr`, `Callback`, … | No — write `use ffi::types::*;` |
| `io` | `Stream`, `IoError`, `Read` / `Write`, `stdin` / `stdout` / `stderr` / `open` / `read` / `write` / `close`, `from_bytes` / `to_bytes`, sync adapters, TCP, UDP | No — write `use io::*;` |

### Prelude rebind / redefine

Short prelude names are bound in scope so `Option::Some` and `T: Eq` work without imports. To redefine a prelude name:

```0s
use prelude::ops::Eq as PreludeEq; // frees short `Eq`
trait Eq<T> { /* your trait */ }   // now allowed
// Builtin still reachable as `prelude::ops::Eq` or `PreludeEq`
```

Without the `as` rebind, `trait Eq` / `enum Option` is a conflict diagnostic.

`zero.toml` `preludes = […]` customization is **not** implemented yet — the compiler always injects `prelude` + `prelude::ops` + `prelude::test`.

---

## Path resolution algorithm

Given a concrete import `use a::b::c;`:

0. If the path matches a **virtual module** export (see above), bind that export and stop — no disk file is loaded.
1. Split the path into segments. All segments except the last form the **directory path**; the last segment is the **item name**.
   - Path: `["a", "b"]`
   - Item name: `"c"`
2. For each search root in `[module].roots` (from `zero.toml`, in declaration order):
   - Build candidate: `<project_root>/<root>/a/b/c.0s`
   - If the file exists, **stop** — this is the resolved module file.
3. If no root contains the file, emit a module-not-found diagnostic.

Given a glob import `use a::b::*;`:

1. Split the path. The segment before `*` is the **module stem**.
   - For `use foo::*`: path = `["foo"]`, stem = `"foo"`
   - For `use a::b::*`: path = `["a"]`, stem = `"b"` (the last non-glob segment names the file)
2. Pop the last segment from the path to get the directory prefix.
3. Resolve `<project_root>/<root>/<path>/<stem>.0s` using the same root search order.
4. Example: `use foo::*` → `<root>/foo.0s`

Given a `mod foo;` declaration:

1. For each search root, check `<project_root>/<root>/foo.0s`.
2. First existing file wins.

### Resolution examples

| Statement | Resolved file (root = `src/`) |
|-----------|-------------------------------|
| `use foo::sadge;` | `src/foo/sadge.0s` |
| `use lib::io::read;` | `src/lib/io/read.0s` |
| `use foo::*;` | `src/foo.0s` |
| `mod foo;` | `src/foo.0s` |

With multiple roots `["./src", "./vendor"]`, the compiler checks `./src/...` first, then `./vendor/...`. The first match wins.

---

## Namespace rules

### Computing a file's namespace

For a resolved file path, the namespace is:

1. Find the **first** search root that contains the file.
2. Take the path relative to that root.
3. Strip the `.0s` extension.
4. Replace path separators with `::`.

Examples (root = `src/`):

| Absolute path | Relative path | Namespace |
|---------------|---------------|-----------|
| `src/foo.0s` | `foo.0s` | `foo` |
| `src/foo/sadge.0s` | `foo/sadge.0s` | `foo::sadge` |
| `src/lib/io/read.0s` | `lib/io/read.0s` | `lib::io::read` |

If a file is outside all search roots, the namespace falls back to the file's bare stem.

### Entry file

The file passed to the compiler (or declared in `[entry].file`) uses the **empty namespace** (`""`). Top-level items in the entry file have unprefixed FQNs.

### Fully qualified names (FQN)

Top-level functions register under:

```
<namespace>::<function_name>
```

If the namespace is empty, the FQN is just `<function_name>`.

For a concrete import `use a::b::c;`:

- Expected file: `<root>/a/b/c.0s`
- File namespace: `a::b::c`
- Expected function name inside the file: `c`
- FQN: `a::b::c::c`

The last path segment names both the file and the function inside it.

---

## Glob semantics

`use foo::*;`:

1. **Discovery:** loads and compiles `foo.0s` (same as a non-glob reference to that file).
2. **Scope:** after the dependency files compile, every top-level function whose FQN starts with `foo::` and has no further `::` segments is imported into the current scope by its bare name.

Example — `src/foo.0s`:

```0s
fn sadge() { print "%i", 100; }
fn greet() { print "%i", 200; }
```

After `use foo::*;` in another file, both `sadge()` and `greet()` are callable directly.

### Glob limitations

- **File-scoped only.** `use foo::*` imports from `foo.0s`. It does **not** import items from `foo/bar.0s` or other files in a `foo/` directory.
- **Top-level items only.** Nested items (if added in future versions) are not glob-imported.
- **No aliasing.** `use foo::* as bar;` is not valid syntax.
- **Compile order matters.** Glob expansion reads the function registry after dependency files compile. The imported file must compile before the consumer.

---

## Aliasing rules

`use path::name as alias;`:

| Property | Behavior |
|----------|----------|
| Local name | `alias` |
| FQN target | `<namespace>::<name>` where namespace = `<path>::<name>` |
| Function expected in file | `fn name()` |
| Typechecker | Inserts `alias` into the environment with a fresh type variable |

Without `as`, the local name defaults to the last path segment (`name`).

Examples:

```0s
use foo::sadge;           // local: sadge  → FQN foo::sadge::sadge
use foo::sadge as f;      // local: f      → FQN foo::sadge::sadge
use lib::io::read as rd;  // local: rd     → FQN lib::io::read::read
```

Aliases are **per-file**. They do not propagate to other modules.

---

## Discovery and compilation order

The pipeline runs in two passes:

### 1. Discovery pass

- Start with the entry file on the worklist.
- Parse each file; walk the AST for `use` and `mod` declarations.
- Enqueue referenced files (deduplicated).
- Repeat until the worklist stabilizes (all transitive dependencies found).
- Cache source text to avoid re-reading from disk.

### 2. Compilation pass

- Drain the worklist in **LIFO** order (dependencies compile before consumers).
- Each file compiles with its computed namespace.
- `use` statements in the consumer file resolve local names to FQNs via the alias map.
- Glob imports expand against the compiled function registry.

---

## Interaction with `zero.toml`

Module resolution depends on `[module].roots` from the project manifest. See [Project configuration](project-config.md) for manifest format and default behavior.

Without a manifest, the compiler uses a single default root: `src/`.

---

## Diagnostics

Common errors:

| Situation | Message (approximate) |
|-----------|----------------------|
| File not found | Module not found for `use a::b::c` |
| Unknown identifier after import | Cannot find function `x` in this scope |
| Function name mismatch | Call fails at link time if FQN not in registry |

The typechecker validates that aliased names exist in the environment. The codegen maps local names to FQNs; a missing target surfaces when the call is emitted.
