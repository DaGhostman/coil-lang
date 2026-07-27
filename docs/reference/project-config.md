# Project configuration (`coil.toml`)

The **`coil.toml`** file at a project's root tells the compiler where to find module files, which git dependencies to vendor, and optionally which file is the entry point.

---

## File location

Place `coil.toml` in the **project root** — the directory the compiler treats as the working directory when resolving relative paths.

```
my-project/
├── coil.toml
├── coil.lock
└── src/
    ├── main.hy
    └── foo/
        └── bar.hy
```

If `coil.toml` is absent, the compiler uses built-in defaults (see [Default behavior](#default-behavior-without-coiltoml)).

---

## Format

The parser accepts a minimal TOML-like subset:

- Section headers: `[module]`, `[entry]`, `[ffi]`, `[package]`, `[dependencies.<name>]`
- Key-value lines: `key = value`
- String values: double-quoted (`"./src"`)
- Array values: `["a", "b"]`
- Comments: `#` to end of line
- Blank lines are ignored

Unknown sections or keys are parse errors.

---

## Sections and keys

### `[package]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `vendor_dir` | string | No (defaults to `"vendor"`) | Directory under the project root where `coil install` materialises git dependencies |

```toml
[package]
vendor_dir = "vendor"
```

### `[module]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `roots` | array of strings | No (defaults to `["src"]`) | Directories searched for module files, relative to the project root |

Example:

```toml
[module]
roots = ["./src", "./builtins"]
```

Each path in `roots` is a **search root**. When resolving `use foo::bar;`, the compiler looks for `<root>/foo/bar.hy` under each root **in order**. The first existing file wins.

If the `[module]` section is omitted entirely, roots default to `["src"]`.

If `[module]` is present but `roots` is omitted, roots also default to `["src"]`.

### `[dependencies.<name>]`

Git dependencies (no central registry). The section name’s last segment is the **package name** and the `use` namespace prefix (`foo` → `foo::…`).

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `git` | string | Yes | Git remote URL (HTTPS or SSH) |
| `version` | string | No (defaults to `"*"`) | Semver requirement (`^1.2`, `~2.0`, `1.0.0`, `*`) |
| `path` | string | No | Subdirectory inside the repo (monorepos) |

```toml
[dependencies.foo]
git = "https://github.com/org/foo"
version = "^1.2.0"

[dependencies.bar]
git = "nina.v@example.com:org/private.git"
version = "~2.0"
path = "packages/bar"
```

Private GitHub HTTPS remotes: set `GH_TOKEN` or `GITHUB_TOKEN`. SSH remotes use your agent/credentials as usual.

Locked versions live in **`coil.lock`** (commit SHA + concrete semver). Sources are checked out under `vendor/<name>/` (or `vendor_dir`). Commit `coil.lock`; keep `vendor/` out of version control (see `.gitignore`).

CLI:

| Command | Meaning |
|---------|---------|
| `coil add <name> <git-url> [--version <req>]` | Append the dependency, resolve a matching tag, vendor, lock |
| `coil install` | Materialise every lock entry into the vendor directory |
| `coil update [name…]` | Propose newer matching tags, print commits since the locked SHA (grouped by repo), confirm, then bump |
| `coil update -y` / `--yes` | Same as `update`, apply without prompting |

Vendored packages are resolved **after** project `roots`. A package’s own `coil.toml` `[module].roots` (default `["src", "."]`) selects files inside the checkout; namespaces are always prefixed with the package name.

`coil install` installs the **full dependency tree**: after vendoring a package it reads that package’s `[dependencies.*]` and installs those too. Conflicting `git`/`path` for the same package name is an error; compatible cycles (same source reached via different parents) are allowed. `coil.lock` is the source of truth for commits; if a newer matching tag exists, install prints a notice and leaves the lock unchanged (`coil update` bumps it). Notices are skipped when `CI` or `COIL_OFFLINE` is set in the environment.

### `[entry]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `file` | string | No | Explicit entry-point file, relative to the project root |

Example:

```toml
[entry]
file = "./src/main.hy"
```

When set, the compiler uses this file as the program entry point (jumps to `main` in that file).

When omitted, the entry file is whatever you pass on the command line:

```bash
cargo run -- examples/modules.hy
```

---

## Complete example

```toml
# coil project manifest

[package]
vendor_dir = "vendor"

[module]
roots = ["./src", "./builtins"]

[dependencies.foo]
git = "https://github.com/org/foo"
version = "^1.0"

[entry]
# file = "./src/main.hy"
```

---

## Discovery algorithm

Given a `use a::b::c;` statement and roots `["./src", "./builtins"]`:

### Step 1 — Split the import path

- Directory path segments: `["a", "b"]`
- Item name (last segment): `"c"`

### Step 2 — Search each root in order

For root `./src`:

```
./src/a/b/c.hy   → exists? use this file
```

If not found, try `./builtins`, then (if `a` is a declared dependency) `vendor/a/…` using that package’s module roots.

### Step 3 — First match wins

Stop at the first path that exists on disk. That file is loaded and compiled.

### Step 4 — Compute namespace

Strip the matching root prefix, remove `.hy`, replace `/` with `::`. For vendored packages, prefix with the package name (`foo::something`).

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
| `[package].vendor_dir` | `"vendor"` |
| `[entry].file` | None — use CLI argument |
| dependencies | none |

This means a minimal project with only `src/main.hy` and `src/foo/bar.hy` works without any manifest, as long as you run the compiler from the project root:

```bash
cargo run -- src/main.hy
```

The namespace test suite confirms that `use foo::greet;` resolves to `src/foo/greet.hy` with no manifest present.

---

## Multiple roots in practice

Use multiple roots for local libraries and builtins; use `[dependencies.*]` for git packages:

```toml
[module]
roots = ["./src", "./builtins"]

[dependencies.http]
git = "https://github.com/example/coil-http"
version = "^0.3"
```

Resolution order means **your source tree takes precedence** over vendored packages when a path exists in both places.

Typical layout:

```
project/
├── coil.toml
├── coil.lock      # commit this
├── src/           # application code (first priority)
├── vendor/        # git checkouts from `coil install` (usually gitignored)
└── builtins/      # local shared helpers
```

---

## Reserved / future keys

Future versions may add keys such as:

```toml
[module]
preludes = ["./stdlib"]   # customize auto-imports (not yet implemented)
strict   = true           # reject undefined names at typecheck (not yet implemented)
```

Compiler builtins (`prelude`, `prelude::ops`, `ffi`, `ffi::types`) are virtual modules owned by the compiler — they are **not** configured via `coil.toml` today. Every file always gets the implicit prelude; FFI still requires an explicit `use`.

---

## Related documentation

- [Modules reference](modules.md) — `use` / `mod` syntax, FQN rules, glob semantics
- [Tutorial: Modules](../tutorial/06-modules.md) — walkthrough with `examples/modules.hy`
