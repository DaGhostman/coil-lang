# Project configuration (`zero.toml`)

The **`zero.toml`** file at a project's root tells the compiler where to find module files and optionally which file is the entry point.

---

## File location

Place `zero.toml` in the **project root** — the directory the compiler treats as the working directory when resolving relative paths.

```
my-project/
├── zero.toml
└── src/
    ├── main.0s
    └── foo/
        └── bar.0s
```

If `zero.toml` is absent, the compiler uses built-in defaults (see [Default behavior](#default-behavior-without-zerotoml)).

---

## Format

The parser accepts a minimal TOML-like subset:

- Section headers: `[module]`, `[entry]`
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

Each path in `roots` is a **search root**. When resolving `use foo::bar;`, the compiler looks for `<root>/foo/bar.0s` under each root **in order**. The first existing file wins.

If the `[module]` section is omitted entirely, roots default to `["src"]`.

If `[module]` is present but `roots` is omitted, roots also default to `["src"]`.

### `[entry]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `file` | string | No | Explicit entry-point file, relative to the project root |

Example:

```toml
[entry]
file = "./src/main.0s"
```

When set, the compiler uses this file as the program entry point (jumps to `main` in that file).

When omitted, the entry file is whatever you pass on the command line:

```bash
cargo run -- examples/modules.0s
```

---

## Complete example

From `zero.toml.example`:

```toml
# zero-script project manifest

[module]
# Search roots for `use` resolution. Each path is relative to
# the directory containing this zero.toml file. The compiler
# searches the roots in order; the first file that exists wins.
roots = ["./src", "./vendor", "./builtins"]

# Default when no zero.toml exists: roots = ["src"]

[entry]
# Optional entry point. If omitted, use the file from the CLI.
# file = "./src/main.0s"
```

---

## Discovery algorithm

Given a `use a::b::c;` statement and roots `["./src", "./vendor", "./builtins"]`:

### Step 1 — Split the import path

- Directory path segments: `["a", "b"]`
- Item name (last segment): `"c"`

### Step 2 — Search each root in order

For root `./src`:

```
./src/a/b/c.0s   → exists? use this file
```

If not found, try `./vendor`:

```
./vendor/a/b/c.0s
```

Then `./builtins`:

```
./builtins/a/b/c.0s
```

### Step 3 — First match wins

Stop at the first path that exists on disk. That file is loaded and compiled.

### Step 4 — Compute namespace

Strip the matching root prefix, remove `.0s`, replace `/` with `::`:

```
./src/a/b/c.0s  →  namespace "a::b::c"
```

### Glob imports

For `use foo::*;`:

1. The module stem is the last non-`*` segment: `"foo"`.
2. Directory prefix is all preceding segments (empty for `use foo::*`).
3. Resolve `<root>/foo.0s` (not a subdirectory).
4. Namespace of `foo.0s` is `foo`.

### `mod` declarations

For `mod foo;`:

1. Search each root for `<root>/foo.0s`.
2. First existing file wins.
3. Namespace is `foo`.

### Transitive discovery

1. Enqueue the entry file.
2. Parse it; find all `use` and `mod` declarations.
3. Enqueue referenced files not yet seen.
4. Repeat until no new files are discovered.
5. Compile all discovered files in dependency order.

---

## Default behavior without `zero.toml`

When no `zero.toml` exists in the project root (or the file cannot be read):

| Setting | Default |
|---------|---------|
| `[module].roots` | `["src"]` |
| `[entry].file` | None — use CLI argument |

This means a minimal project with only `src/main.0s` and `src/foo/bar.0s` works without any manifest, as long as you run the compiler from the project root:

```bash
cargo run -- src/main.0s
```

The namespace test suite confirms that `use foo::greet;` resolves to `src/foo/greet.0s` with no manifest present.

---

## Multiple roots in practice

Use multiple roots to vendored or built-in libraries:

```toml
[module]
roots = ["./src", "./vendor", "./builtins"]
```

Resolution order means **your source tree takes precedence**. If both `src/foo/greet.0s` and `vendor/foo/greet.0s` exist, the `src/` copy is used.

Typical layout:

```
project/
├── zero.toml
├── src/           # application code (first priority)
├── vendor/        # third-party zero-script modules
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

Compiler builtins (`prelude`, `prelude::ops`, `ffi`, `ffi::types`) are virtual modules owned by the compiler — they are **not** configured via `zero.toml` today. Every file always gets the implicit prelude; FFI still requires an explicit `use`.

---

## Related documentation

- [Modules reference](modules.md) — `use` / `mod` syntax, FQN rules, glob semantics
- [Tutorial: Modules](../tutorial/06-modules.md) — walkthrough with `examples/modules.0s`
