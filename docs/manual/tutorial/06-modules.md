# Tutorial: Modules

As programs grow, a single file becomes hard to navigate. coil supports **multi-file projects** with a module system based on `use` imports, `mod` forward declarations, and a `coil.toml` project manifest.

---

## Why modules?

Modules let you:

- Split code across files with clear boundaries
- Reuse functions without copying source
- Organize code by feature or layer (`lib::io`, `foo::greet`, and so on)
- Control what names are visible in each file

The compiler discovers dependencies automatically: when one file `use`s another, the pipeline loads and compiles the dependency before the consumer.

---

## Project layout and `coil.toml`

Multi-file projects typically look like this:

```
my-project/
├── coil.toml          # project manifest (optional)
└── src/
    ├── main.hy        # entry point
    └── foo/
        └── sadge.hy   # module file
```

The **`coil.toml`** manifest declares where the compiler searches for module files and optionally sets the entry point. If no manifest exists, the compiler defaults to a single search root: `src/`.

See [Project configuration reference](../../references/project-config.md) for the full format.

---

## File → namespace convention

Every `.hy` file under a search root gets a **namespace** derived from its path:

| File path (under root) | Namespace |
|------------------------|-----------|
| `src/foo.hy` | `foo` |
| `src/foo/sadge.hy` | `foo::sadge` |
| `src/lib/io/read.hy` | `lib::io::read` |

Rules:

- Strip the `.hy` extension
- Replace `/` with `::`
- The path is **relative to the matching search root**, not the project root

### Entry file exception

The file you compile (the **entry file**) lives in the **empty namespace** — its top-level functions have no prefix. If you compile `src/main.hy`, then `fn main()` is just `main`, not `main::main`.

Dependency files loaded via `use` or `mod` get namespaces from their path as shown above.

---

## Importing with `use`

### Concrete import

```coil
use foo::sadge;
```

This statement:

1. Locates the file `<root>/foo/sadge.hy` (searching each root in order)
2. Compiles that file if not already loaded
3. Brings the name `sadge` into the current scope

The imported item is expected to be a top-level function (or other top-level item) **with the same name as the last path segment**. The file `foo/sadge.hy` should define `fn sadge()`.

Call it by the local name:

```coil
sadge();
```

At the call site, the compiler resolves `sadge` to the fully qualified name (FQN) `foo::sadge::sadge`.

### Multi-segment paths

Deeper paths walk into subdirectories:

```coil
use lib::io::read;
```

Resolves to `<root>/lib/io/read.hy`. The function inside should be named `read`, with FQN `lib::io::read::read`.

### Aliasing

Rename an import to avoid collisions or improve readability:

```coil
use foo::sadge as f;

fn main() {
    f();   // calls foo::sadge::sadge
}
```

The `as` clause binds a local name; the underlying FQN is unchanged.

### Glob import

Import **every top-level item** from a module file:

```coil
use foo::*;
```

This loads `foo.hy` (note: the file is `foo.hy`, not a directory) and brings all of its top-level functions into scope by their bare names:

```coil
use foo::*;

fn main() {
    sadge();   // from foo.hy
    greet();   // from foo.hy
}
```

Glob imports are **file-scoped**. They do not reach into subdirectories — `use foo::*` imports from `foo.hy` only, not from `foo/bar.hy`.

### Brace-group import

Import several named items from one module without repeating the path:

```coil
use math::{add, mul as product};
use io::{stdout, write_all};
use string::{format, to_bytes};

fn main() {
    write_all(stdout(), to_bytes(format("%i", add(2, 3))));
    write_all(stdout(), to_bytes(format("%i", product(4, 5))));
}
```

When the items live in `math.hy` (and there is no `math/add.hy`), the resolver falls back to the module file so both names bind correctly.

---

## Forward declarations with `mod`

```coil
mod foo;
```

A `mod` declaration tells the pipeline to load `<root>/foo.hy` but does **not** import any names into the current scope. Use it when you need a file compiled (for side effects or to satisfy link order) without bringing its items into scope.

For most cases, prefer `use` when you need to call functions from another file.

---

## Walkthrough: `examples/modules.hy`

The repository includes a minimal multi-file example.

**Project layout:**

```
examples/
├── modules.hy              ← entry file (empty namespace)
└── src/
    └── foo/
        └── sadge.hy        ← dependency (namespace foo::sadge)
```

**`examples/src/foo/sadge.hy`:**

```coil
use io::{stdout, write_all};
use string::{format, to_bytes};
fn sadge() {
    write_all(stdout(), to_bytes(format("%x\n", 420)));
}
```

**`examples/modules.hy`:**

```coil
use foo::sadge;
use io::{stdout, write_all};
use string::{format, to_bytes};

fn main() {
    sadge();
    write_all(stdout(), to_bytes(format("%x\n", 69)));
}
```

What happens when you run `cargo run -- examples/modules.hy`:

1. The pipeline treats `modules.hy` as the entry file (namespace `""`).
2. Parsing finds `use foo::sadge;` and enqueues `src/foo/sadge.hy`.
3. The discovery pass loads all dependencies transitively.
4. Dependencies compile first (LIFO worklist order).
5. `sadge.hy` compiles with namespace `foo::sadge`; its function registers as FQN `foo::sadge::sadge`.
6. `modules.hy` compiles; the `use` statement maps local `sadge` → `foo::sadge::sadge`.
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
| `src/foo/sadge.hy` | `fn sadge()` | `foo::sadge::sadge` |
| `src/lib/io/read.hy` | `fn read()` | `lib::io::read::read` |
| Entry file `main.hy` | `fn main()` | `main` |

The last segment of a `use` path names **both** the file (`<path>/<name>.hy`) and the expected function name inside that file.

When you write `use foo::sadge;`, the compiler expects:

- File: `<root>/foo/sadge.hy`
- Function: `fn sadge()` inside that file
- FQN: `foo::sadge::sadge`

You normally call imported items by their local alias (`sadge()`), but the FQN is what the bytecode linker uses internally.

---

## Quick reference

| Statement | Loads file | Imports names |
|-----------|------------|---------------|
| `use foo::bar;` | `<root>/foo/bar.hy` | `bar` (local = `bar`) |
| `use foo::bar as baz;` | `<root>/foo/bar.hy` | `baz` (local alias) |
| `use foo::*;` | `<root>/foo.hy` | all top-level items from that file |
| `mod foo;` | `<root>/foo.hy` | none |

For complete syntax rules, path resolution details, and edge cases, see the [Modules reference](../../references/modules.md).

---

## See also

- [Project configuration reference](../../references/project-config.md) — full `coil.toml` format
- [Examples catalog](../examples.md) — `modules.hy` setup notes
- [FFI](07-ffi.md) — next chapter for C interop
