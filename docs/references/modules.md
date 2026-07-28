# Modules reference

This document specifies the syntax and semantics of coil's module system: `use` imports, `mod` forward declarations, namespace rules, and path resolution.

---

## Syntax

### `use` statement

```
use_stmt   ::= 'use' use_path ';'
use_path   ::= IDENT ('::' IDENT)* '::' '{' use_item (',' use_item)* ','? '}'
             | IDENT ('::' IDENT)* ('::' '*')? ('as' IDENT)?
use_item   ::= IDENT ('as' IDENT)?
```

Forms:

| Form | Example |
|------|---------|
| Concrete import | `use foo::sadge;` |
| Aliased import | `use foo::sadge as f;` |
| Glob import | `use foo::*;` |
| Brace group | `use math::{add, mul as product};` |
| Multi-segment | `use lib::io::read;` |

Rules:

- Every `use` statement ends with `;`.
- Path segments are identifiers separated by `::`.
- The last segment is either an identifier (concrete import), `*` (glob), or a `{ … }` brace group.
- Concrete imports and brace-group items may use `as`. Glob imports cannot be aliased.
- A glob marker (`*`) or brace group must be the **last** segment.
- Brace groups desugar to one concrete import per item (same module path).

### `mod` statement

```
mod_stmt ::= 'mod' IDENT ';'
```

Example: `mod foo;`

A `mod` declaration loads a module file but does not bind any names in the current scope.

---

## Virtual modules (compiler builtins)

Some `use` paths resolve to **compiler-owned virtual modules**, not `.hy` files on disk. The pipeline skips disk discovery for these paths.

| Module | Exports | Auto-imported? |
|--------|---------|----------------|
| `prelude` | `Option`, `Result`, `Iterator`, `IntoIterator`, `ArrayIter` | Yes (every file) |
| `prelude::ops` | `Add`, `Sub`, `Mul`, `Div`, `Num`, `Eq`, `Ord`, `Lt`, `Le`, `Gt`, `Ge`, `Show`, `Into` | Yes (every file) |
| `prelude::test` | `assert` | Yes (every file) |
| `ffi` | `dload`, `declare`, `invoke` | No — write `use ffi::*;` |
| `ffi::types` | `Int`, `Float`, `String`, `Void`, `Ptr`, `Callback`, … | No — write `use ffi::types::*;` |
| `io` | `Stream`, `IoError`, `Read` / `Write`, `stdin` / `stdout` / `stderr` / `open` / `read` / `write` / `close`, `from_bytes` / `to_bytes`, sync adapters | No — write `use io::*;` |
| `io::net::tcp` | `connect` / `listen` / `accept` / `accept_wait` | No — write `use io::net::tcp::*;` |
| `io::net::udp` | `bind` / `connect` / `send_to` / `recv_from` / `recv_from_wait` / `local_port` | No — write `use io::net::udp::*;` |
| `io::net::tls` | `connect` / `connect_insecure` | No — write `use io::net::tls::*;` (feature `tls`) |
| `io::fs` | Path/metadata helpers (`exists`, `realpath`, `list_dir`, …) | No — `use io::fs::*;` |
| `time` | `timestamp`, `Period`, `format` / `parse`, monotonic `Instant` | No — `use time::*;` |
| `env` | `args`, `var`, `cwd`, `exit`, `exec` (argv-only) | No — `use env::*;` |
| `crypto` | Hashes, HMAC, AEAD, Ed25519, Argon2, `random_bytes`, … | No — `use crypto::*;` |
| `regex` | PCRE2 `compile` / `is_match` / `find` / `captures` / `split` / `replace` | No — `use regex::*;` |
| `thread` | `spawn`, channels, mutexes | No — `use thread::*;` |

### Prelude rebind / redefine

Short prelude names are bound in scope so `Option::Some` and `T: Eq` work without imports. To redefine a prelude name:

```coil
use prelude::ops::Eq as PreludeEq; // frees short `Eq`
trait Eq<T> { /* your trait */ }   // now allowed
// Builtin still reachable as `prelude::ops::Eq` or `PreludeEq`
```

Without the `as` rebind, `trait Eq` / `enum Option` is a conflict diagnostic.

`coil.toml` `preludes = […]` customization is **not** implemented yet — the compiler always injects `prelude` + `prelude::ops` + `prelude::test`.

---

## Path resolution algorithm

Given a concrete import `use a::b::c;`:

0. If the path matches a **virtual module** export (see above), bind that export and stop — no disk file is loaded.
1. Split the path into segments. All segments except the last form the **directory path**; the last segment is the **item name**.
   - Path: `["a", "b"]`
   - Item name: `"c"`
2. For each search root in `[module].roots` (from `coil.toml`, in declaration order):
   - **One-item-per-file:** `<project_root>/<root>/a/b/c.hy`
   - If the file exists, **stop** — this is the resolved module file.
3. If no one-item-per-file candidate exists, try the **module-file** fallback for each root:
   - `<project_root>/<root>/a/b.hy` (items exported from the module file)
   - This is what makes `use math::add;` work when `add` lives in `math.hy`.
4. If no root contains either form, emit a module-not-found diagnostic.

**Shadowing:** when both `<root>/a/b/c.hy` (one-item-per-file) and `<root>/a/b.hy` (module file) exist, the one-item-per-file path always wins. Avoid keeping the same item name in both layouts.

Given a brace-group import `use math::{add, mul};`:

1. Desugar to `use math::add;` + `use math::mul;` (same path, one item each).
2. Resolve each item with the algorithm above (typically both hit the same `math.hy`).

Given a glob import `use a::b::*;`:

1. Split the path. The segment before `*` is the **module stem**.
   - For `use foo::*`: path = `["foo"]`, stem = `"foo"`
   - For `use a::b::*`: path = `["a"]`, stem = `"b"` (the last non-glob segment names the file)
2. Pop the last segment from the path to get the directory prefix.
3. Resolve `<project_root>/<root>/<path>/<stem>.hy` using the same root search order.
4. Example: `use foo::*` → `<root>/foo.hy`

Given a `mod foo;` declaration:

1. For each search root, check `<project_root>/<root>/foo.hy`.
2. First existing file wins.

### Resolution examples

| Statement | Resolved file (root = `src/`) |
|-----------|-------------------------------|
| `use foo::sadge;` | `src/foo/sadge.hy`, else `src/foo.hy` |
| `use math::{add, mul};` | `src/math.hy` (module-file fallback) |
| `use lib::io::read;` | `src/lib/io/read.hy`, else `src/lib/io.hy` |
| `use foo::*;` | `src/foo.hy` |
| `mod foo;` | `src/foo.hy` |

With multiple roots `["./src", "./vendor"]`, the compiler checks `./src/...` first, then `./vendor/...`. The first match wins.

### Shipping / consuming `stdlib`

The coil workspace manifest includes `./stdlib` in `[module].roots` so programs
can `use http::client::*;` (and future stdlib packages) without vendoring by
hand. Project manifests should list the same root (or a path to a checkout):

```toml
[module]
roots = ["./src", "./stdlib"]
```

See [HTTP/1.1 client](../manual/http-client.md) for the HTTP API.

---

## Namespace rules

### Computing a file's namespace

For a resolved file path, the namespace is:

1. Find the **first** search root that contains the file.
2. Take the path relative to that root.
3. Strip the `.hy` extension.
4. Replace path separators with `::`.

Examples (root = `src/`):

| Absolute path | Relative path | Namespace |
|---------------|---------------|-----------|
| `src/foo.hy` | `foo.hy` | `foo` |
| `src/foo/sadge.hy` | `foo/sadge.hy` | `foo::sadge` |
| `src/lib/io/read.hy` | `lib/io/read.hy` | `lib::io::read` |

If a file is outside all search roots, the namespace falls back to the file's bare stem.

### Entry file

The file passed to the compiler (or declared in `[entry].file`) uses the **empty namespace** (`""`). Top-level items in the entry file have unprefixed FQNs.

### Fully qualified names (FQN)

Top-level functions register under:

```
<namespace>::<function_name>
```

If the namespace is empty, the FQN is just `<function_name>`.

The FQN shape depends on **which file** path resolution loaded (see [Path resolution algorithm](#path-resolution-algorithm)):

**Convention A — one-item-per-file** (`use a::b::c;` → `<root>/a/b/c.hy`):

- File namespace: `a::b::c`
- Function name inside the file: `c`
- FQN: `a::b::c::c` (last path segment names both the file and the function)

**Convention B — module-file fallback** (`use math::add;` → `<root>/math.hy`):

- File namespace: `math`
- Function name inside the file: `add`
- FQN: `math::add` (namespace is the module file stem; item is the bare function name)

---

## Glob semantics

`use foo::*;`:

1. **Discovery:** loads and compiles `foo.hy` (same as a non-glob reference to that file).
2. **Scope:** after the dependency files compile, every top-level function whose FQN starts with `foo::` and has no further `::` segments is imported into the current scope by its bare name.

Example — `src/foo.hy`:

```coil
fn sadge() { print "%i", 100; }
fn greet() { print "%i", 200; }
```

After `use foo::*;` in another file, both `sadge()` and `greet()` are callable directly.

### Glob limitations

- **File-scoped only.** `use foo::*` imports from `foo.hy`. It does **not** import items from `foo/bar.hy` or other files in a `foo/` directory.
- **Top-level items only.** Nested items (if added in future versions) are not glob-imported.
- **No aliasing.** `use foo::* as bar;` is not valid syntax.
- **Compile order matters.** Glob expansion reads the function registry after dependency files compile. The imported file must compile before the consumer.

---

## Aliasing rules

`use path::name as alias;`:

| Property | Behavior |
|----------|----------|
| Local name | `alias` |
| FQN target | Depends on resolved file (Convention A vs B; see above) |
| Function expected in file | `fn name()` |
| Typechecker | Inserts `alias` into the environment with a fresh type variable |

Without `as`, the local name defaults to the last path segment (`name`).

Examples:

```coil
// Convention A — foo/sadge.hy
use foo::sadge;           // local: sadge  → FQN foo::sadge::sadge
use foo::sadge as f;      // local: f      → FQN foo::sadge::sadge
use lib::io::read as rd;  // local: rd     → FQN lib::io::read::read

// Convention B — math.hy (no math/add.hy)
use math::add;            // local: add    → FQN math::add
use math::add as plus;    // local: plus   → FQN math::add
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
- Multi-file programs share **one constant pool** for the whole link (`?` / match / other pool-backed immediates). The pool is cleared only on a fresh compile (prologue-only bytecode), not between modules.

---

## Interaction with `coil.toml`

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
