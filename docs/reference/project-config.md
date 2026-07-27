# Project configuration (`coil.toml`)

The **`coil.toml`** file at a project's root tells the compiler where to find module files and optionally which file is the entry point.

---

## File location

Place `coil.toml` in the **project root** — the directory the compiler treats as the working directory when resolving relative paths.

```
my-project/
├── coil.toml
└── src/
    ├── main.hy
    └── foo/
        └── bar.hy
```

If `coil.toml` is absent, the compiler uses built-in defaults (see [Default behavior](#default-behavior-without-coiltoml)).

---

## Format

The parser accepts a minimal TOML-like subset:

- Section headers: `[module]`, `[entry]`, `[env]`
- Key-value lines: `key = value`
- String values: double-quoted (`"./src"`)
- Array values: `["a", "b"]`
- Comments: `#` to end of line
- Blank lines are ignored

Unknown sections or keys are parse errors.

---

## Sections and keys

### `[module]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `roots` | array of strings | No (defaults to `["src"]`) | Directories searched for module files, relative to the project root |

Example:

```toml
[module]
roots = ["./src", "./vendor", "./builtins"]
```

Each path in `roots` is a **search root**. When resolving `use foo::bar;`, the compiler looks under each root **in order** for `<root>/foo/bar.hy` (one-item-per-file), then falls back to `<root>/foo.hy` (module file). The first existing path wins; see [Discovery algorithm](#discovery-algorithm).

If the `[module]` section is omitted entirely, roots default to `["src"]`.

If `[module]` is present but `roots` is omitted, roots also default to `["src"]`.

### `[entry]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `file` | string | No | Explicit entry-point file, relative to the project root |

Example:

```toml
[entry]
file = "./src/main.hy"
```

When set, `coil` and `coil compile` with **no file argument** use this path as the program entry (relative to the project root that owns `coil.toml`).

When omitted, you must pass the entry file on the command line:

```bash
coil examples/modules.hy
# or
coil compile examples/modules.hy
```

```bash
# with [entry] file = "./src/main.hy" in coil.toml:
coil
coil compile
```

### `[env]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `allow_exec` | bool | No (defaults to `true`) | When `false`, `env::exec` fails at runtime with `ExecDisabled` (the compiler still warns at compile time) |

Example:

```toml
[env]
allow_exec = false
```

---

## Complete example

From `coil.toml.example`:

```toml
# coil project manifest

[module]
# Search roots for `use` resolution. Each path is relative to
# the directory containing this coil.toml file. The compiler
# searches the roots in order; the first file that exists wins.
roots = ["./src", "./vendor", "./builtins"]

# Default when no coil.toml exists: roots = ["src"]

[entry]
# Optional entry point. If omitted, use the file from the CLI.
# file = "./src/main.hy"
```

---

## Discovery algorithm

Given a `use a::b::c;` statement and roots `["./src", "./vendor", "./builtins"]`.
Resolution matches the [modules reference](modules.md#path-resolution-algorithm): **one-item-per-file first**, then a **module-file** fallback. When both `<root>/foo/item.hy` and `<root>/foo.hy` exist, the item file wins (see shadowing note in modules.md). **Migration:** if a project accidentally kept both layouts, `use foo::item` silently binds the item file after this change — delete or rename the unused path.

### Step 1 — Split the import path

- Directory path segments: `["a", "b"]`
- Item name (last segment): `"c"`

### Step 2 — Search each root in order (Convention A)

For root `./src`:

```
./src/a/b/c.hy   → exists? use this file
```

If not found, try `./vendor`:

```
./vendor/a/b/c.hy
```

Then `./builtins`:

```
./builtins/a/b/c.hy
```

### Step 2b — Module-file fallback (Convention B)

If no one-item-per-file candidate exists in any root, try each root again for the parent module file (directory path only):

```
./src/a/b.hy     → exists? use this file (item `c` lives inside)
./vendor/a/b.hy
./builtins/a/b.hy
```

Example: `use math::add;` with only `./src/math.hy` present resolves to that file (namespace `math`, FQN `math::add`).

### Step 3 — First match wins

Stop at the first path that exists on disk. That file is loaded and compiled.

### Step 4 — Compute namespace

Strip the matching root prefix, remove `.hy`, replace `/` with `::`:

```
./src/a/b/c.hy  →  namespace "a::b::c"
./src/a/b.hy    →  namespace "a::b"     (module-file fallback)
```

### Glob imports

For `use foo::*;`:

1. The module stem is the last non-`*` segment: `"foo"`.
2. Directory prefix is all preceding segments (empty for `use foo::*`).
3. Resolve `<root>/foo.hy` (not a subdirectory).
4. Namespace of `foo.hy` is `foo`.

### `mod` declarations

For `mod foo;`:

1. Search each root for `<root>/foo.hy`.
2. First existing file wins.
3. Namespace is `foo`.

### Transitive discovery

1. Enqueue the entry file.
2. Parse it; find all `use` and `mod` declarations.
3. Enqueue referenced files not yet seen.
4. Repeat until no new files are discovered.
5. Compile all discovered files in dependency order.

---

## Default behavior without `coil.toml` {#default-behavior-without-coiltoml}

When no `coil.toml` exists in the project root (or the file cannot be read):

| Setting | Default |
|---------|---------|
| `[module].roots` | `["src"]` |
| `[entry].file` | None — use CLI argument |

This means a minimal project with only `src/main.hy` and `src/foo/bar.hy` works without any manifest, as long as you run the compiler from the project root:

```bash
cargo run -- src/main.hy
```

The namespace test suite confirms that `use foo::greet;` resolves to `src/foo/greet.hy` with no manifest present.

---

## Multiple roots in practice

Use multiple roots to vendored or built-in libraries:

```toml
[module]
roots = ["./src", "./vendor", "./builtins"]
```

Resolution order means **your source tree takes precedence**. If both `src/foo/greet.hy` and `vendor/foo/greet.hy` exist, the `src/` copy is used.

Typical layout:

```
project/
├── coil.toml
├── src/           # application code (first priority)
├── vendor/        # third-party coil modules
└── builtins/      # compiler-shipped helpers (e.g. FFI wrappers)
```

---

## Reserved / future keys

The parser currently accepts only the keys documented above. Future versions may add keys such as:

```toml
[module]
preludes = ["./stdlib"]   # customize auto-imports (not yet implemented)
strict   = true           # reject undefined names at typecheck (not yet implemented)
```

These are recognized in planning documents but **ignored or rejected** by the current parser. Do not rely on them.

Compiler builtins (`prelude`, `prelude::ops`, `ffi`, `ffi::types`) are virtual modules owned by the compiler — they are **not** configured via `coil.toml` today. Every file always gets the implicit prelude; FFI still requires an explicit `use`.

---

## Related documentation

- [Modules reference](modules.md) — `use` / `mod` syntax, FQN rules, glob semantics
- [Tutorial: Modules](../tutorial/06-modules.md) — walkthrough with `examples/modules.hy`
