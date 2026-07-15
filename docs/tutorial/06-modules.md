# Tutorial: Modules

As programs grow, a single file becomes hard to navigate. zero-script supports **multi-file projects** with a module system based on `use` imports, `mod` forward declarations, and a `zero.toml` project manifest.

---

## Why modules?

Modules let you:

- Split code across files with clear boundaries
- Reuse functions without copying source
- Organize code by feature or layer (`lib::io`, `foo::greet`, and so on)
- Control what names are visible in each file

The compiler discovers dependencies automatically: when one file `use`s another, the pipeline loads and compiles the dependency before the consumer.

---

## Project layout and `zero.toml`

Multi-file projects typically look like this:

```
my-project/
├── zero.toml          # project manifest (optional)
└── src/
    ├── main.0s        # entry point
    └── foo/
        └── sadge.0s   # module file
```

The **`zero.toml`** manifest declares where the compiler searches for module files and optionally sets the entry point. If no manifest exists, the compiler defaults to a single search root: `src/`.

See [Project configuration reference](../reference/project-config.md) for the full format.

---

## File → namespace convention

Every `.0s` file under a search root gets a **namespace** derived from its path:

| File path (under root) | Namespace |
|------------------------|-----------|
| `src/foo.0s` | `foo` |
| `src/foo/sadge.0s` | `foo::sadge` |
| `src/lib/io/read.0s` | `lib::io::read` |

Rules:

- Strip the `.0s` extension
- Replace `/` with `::`
- The path is **relative to the matching search root**, not the project root

### Entry file exception

The file you compile (the **entry file**) lives in the **empty namespace** — its top-level functions have no prefix. If you compile `src/main.0s`, then `fn main()` is just `main`, not `main::main`.

Dependency files loaded via `use` or `mod` get namespaces from their path as shown above.

---

## Importing with `use`

### Concrete import

```0s
use foo::sadge;
```

This statement:

1. Locates the file `<root>/foo/sadge.0s` (searching each root in order)
2. Compiles that file if not already loaded
3. Brings the name `sadge` into the current scope

The imported item is expected to be a top-level function (or other top-level item) **with the same name as the last path segment**. The file `foo/sadge.0s` should define `fn sadge()`.

Call it by the local name:

```0s
sadge();
```

At the call site, the compiler resolves `sadge` to the fully qualified name (FQN) `foo::sadge::sadge`.

### Multi-segment paths

Deeper paths walk into subdirectories:

```0s
use lib::io::read;
```

Resolves to `<root>/lib/io/read.0s`. The function inside should be named `read`, with FQN `lib::io::read::read`.

### Aliasing

Rename an import to avoid collisions or improve readability:

```0s
use foo::sadge as f;

fn main() {
    f();   // calls foo::sadge::sadge
}
```

The `as` clause binds a local name; the underlying FQN is unchanged.

### Glob import

Import **every top-level item** from a module file:

```0s
use foo::*;
```

This loads `foo.0s` (note: the file is `foo.0s`, not a directory) and brings all of its top-level functions into scope by their bare names:

```0s
use foo::*;

fn main() {
    sadge();   // from foo.0s
    greet();   // from foo.0s
}
```

Glob imports are **file-scoped**. They do not reach into subdirectories — `use foo::*` imports from `foo.0s` only, not from `foo/bar.0s`.

---

## Forward declarations with `mod`

```0s
mod foo;
```

A `mod` declaration tells the pipeline to load `<root>/foo.0s` but does **not** import any names into the current scope. Use it when you need a file compiled (for side effects or to satisfy link order) without bringing its items into scope.

For most cases, prefer `use` when you need to call functions from another file.

---

## Walkthrough: `examples/modules.0s`

The repository includes a minimal multi-file example.

**Project layout:**

```
examples/
├── modules.0s              ← entry file (empty namespace)
└── src/
    └── foo/
        └── sadge.0s        ← dependency (namespace foo::sadge)
```

**`examples/src/foo/sadge.0s`:**

```0s
fn sadge() {
    print "%x\n", 420;
}
```

**`examples/modules.0s`:**

```0s
use foo::sadge;

fn main() {
    sadge();
    print "%x\n", 69;
}
```

What happens when you run `cargo run -- examples/modules.0s`:

1. The pipeline treats `modules.0s` as the entry file (namespace `""`).
2. Parsing finds `use foo::sadge;` and enqueues `src/foo/sadge.0s`.
3. The discovery pass loads all dependencies transitively.
4. Dependencies compile first (LIFO worklist order).
5. `sadge.0s` compiles with namespace `foo::sadge`; its function registers as FQN `foo::sadge::sadge`.
6. `modules.0s` compiles; the `use` statement maps local `sadge` → `foo::sadge::sadge`.
7. `main()` calls `sadge()` (prints `420` in hex as `1a4`) then prints `69` in hex as `45`.

Expected output: `1a4\n45`.

---

## Fully qualified names (FQN)

Every top-level function in a dependency file gets an FQN:

```
<namespace>::<function_name>
```

Examples:

| File | Function | FQN |
|------|----------|-----|
| `src/foo/sadge.0s` | `fn sadge()` | `foo::sadge::sadge` |
| `src/lib/io/read.0s` | `fn read()` | `lib::io::read::read` |
| Entry file `main.0s` | `fn main()` | `main` |

The last segment of a `use` path names **both** the file (`<path>/<name>.0s`) and the expected function name inside that file.

When you write `use foo::sadge;`, the compiler expects:

- File: `<root>/foo/sadge.0s`
- Function: `fn sadge()` inside that file
- FQN: `foo::sadge::sadge`

You normally call imported items by their local alias (`sadge()`), but the FQN is what the bytecode linker uses internally.

---

## Quick reference

| Statement | Loads file | Imports names |
|-----------|------------|---------------|
| `use foo::bar;` | `<root>/foo/bar.0s` | `bar` (local = `bar`) |
| `use foo::bar as baz;` | `<root>/foo/bar.0s` | `baz` (local alias) |
| `use foo::*;` | `<root>/foo.0s` | all top-level items from that file |
| `mod foo;` | `<root>/foo.0s` | none |

For complete syntax rules, path resolution details, and edge cases, see the [Modules reference](../reference/modules.md).

---

## See also

- [Project configuration reference](../reference/project-config.md) — full `zero.toml` format
- [Examples catalog](../examples.md) — `modules.0s` setup notes
- [FFI](07-ffi.md) — next chapter for C interop
