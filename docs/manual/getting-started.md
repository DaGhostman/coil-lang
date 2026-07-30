# Getting Started

This guide walks you through building coil, running your first program, and understanding how source becomes bytecode on the VM.

## Prerequisites

### Rust toolchain

coil is a Rust workspace. Install a recent stable Rust toolchain ([rustup](https://rustup.rs/)) and ensure `cargo` is on your `PATH`.

```bash
rustc --version
cargo --version
```

### libffi (optional, for FFI examples)

Examples that call C code (`examples/strlen.hy`, `examples/ffi_sum.hy`) require **libffi** at link time.

| Platform | Package |
|----------|---------|
| Arch Linux | `libffi` |
| Debian / Ubuntu | `libffi-dev` |
| Fedora | `libffi-devel` |

You can build and run all non-FFI examples without libffi.

### libpcre2 (optional, for `regex` examples)

The virtual `regex` module links **libpcre2** via the `pcre2` / `pcre2-sys` crates (`examples/regex_demo.hy`).

| Platform | Package |
|----------|---------|
| Arch Linux | `pcre2` |
| Debian / Ubuntu | `libpcre2-dev` |
| Fedora | `pcre2-devel` |

If the system library is missing, `pcre2-sys` may build PCRE2 from source (needs a C toolchain).

### Optional Cargo features (`crypto` / `time` / `regex`)

The default build enables the virtual `crypto`, `time`, and `regex` modules (and their crate dependencies). Embedders can strip them:

```toml
machine = { path = "...", default-features = false, features = ["time"] }
# or: features = ["crypto"] / ["regex"]
```

The `compiler` and root `coil` crates mirror the same feature names. With a feature disabled, the corresponding `use crypto::*` / `use time::*` / `use regex::*` will not resolve.

## Build the project

Clone the repository and build the workspace from the root:

```bash
git clone git@github.com:DaGhostman/coil-lang.git
cd coil-lang
cargo build --workspace
```

The GitHub repository is named **`coil-lang`**. If you previously cloned **`zero`** / **zero-script**, update the remote after the repository is renamed on GitHub, or clone fresh:

```bash
git remote set-url origin git@github.com:DaGhostman/coil-lang.git
```

For optimized binaries:

```bash
cargo build --release --workspace
```

A successful build produces the `coil` binary (via `src/main.rs`) plus the `parser`, `compiler`, `machine`, and `common` crates as libraries.

## Run your first program

The canonical starter example computes the 10th Fibonacci number recursively
(fast enough for debug builds; the release CPU / dispatch entry is the same
workload in `examples/fib_bench.hy`):

```coil
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }

    return fib(n - 1) + fib(n - 2);
}

fn main() {
    print "%i", fib(10);
}
```

Run it:

```bash
cargo run -- examples/fib.hy
```

**Expected output:** `55`

The default CLI invocation compiles `examples/fib.hy` to bytecode, serializes it into `out.hyc` in the current directory (if needed), then loads and executes it on the VM.

### CLI commands

| Invocation | Meaning |
|------------|---------|
| `coil [<file.hy>]` | Compile to `out.hyc` (cached) and run; omit the file to use `[entry].file` from `coil.toml` |
| `coil compile [<file.hy>] [-o path]` | Compile only; default output is `out.hyc`; omit the file to use `[entry].file` |
| `coil run <file.hyc>` | Execute a previously compiled archive |
| `coil package <file.hy> [-o path]` | Build a **single executable** for this OS/arch (embedded `.hyc`); always requires an explicit `.hy` path (does not read `[entry].file`) |
| `coil test [path] [--fail-fast]` | Compile and run every `.hy` under `[path]` (default `./tests`) |

Examples:

```bash
# Compile only, custom archive path
cargo run -- compile examples/fib.hy -o /tmp/fib.hyc

# Run that archive
cargo run -- run /tmp/fib.hyc

# Single-file app for this machine (no separate .hyc or coil install needed to run)
cargo run --release -- package examples/fib.hy -o ./fib-app
./fib-app

# With FFI: verify required shared libraries exist on this machine before shipping
cargo run --release -- package examples/strlen.hy -o ./strlen-app --check-native

# Project tests (default root ./tests)
cargo run -- test
cargo run -- test ./tests
cargo run -- test --fail-fast   # stop after the first failed case
```

Layout under `./tests`:

| Path | Meaning |
|------|---------|
| `tests/**/*.hy` (except below) | Must compile; each `test("…")` or `#[test]` case must return `Ok` |
| `tests/compile_fail/**/*.hy` | Must **fail** to compile (negative syntax / type tests) |
| `tests/positive/`, `tests/negative_runtime/` | Organized positive and soft-failure runtime suites |

A test file can declare multiple cases without `fn main`:

```coil
test("addition works") {
    assert(1 + 1 == 2)?;
}

#[test]
fn multiply_works() {
    assert(3 * 4 == 12)?;
}
```

Each `test("…") { … }` or `#[test]` function body runs in Result mode. A failed `assert`/`?` or a language `panic` fails that case; by default the harness continues to the next case (each case runs in an isolated VM so a panic does not skip later cases). Failures print `> Test "<description>" failed`. Files without `test(...)` cases still use a single `fn main()` as one opaque case. Files under `compile_fail/` pass only when compilation returns a clean diagnostic rejection (`Err`); a compiler panic does not count (and aborts under release `panic = "abort"`). Diagnostics for those files are silenced so the summary stays readable.

**Production builds** (`cargo run -- file.hy`, `coil compile`) **omit** harness declarations by default — `test("…")` blocks and `#[test]` functions are stripped before codegen so they are not shipped in `out.hyc`. Use `--include-tests` to embed them (useful when you want to run the harness against a pre-built archive on another machine):

```bash
coil --include-tests compile myapp.hy -o myapp.hyc
coil test ./tests          # always includes harness tests
```

### Recompiling after changes

The default (`BuildAndRun`) path caches `out.hyc`. After editing a `.hy` file, delete the archive to pick up changes, or rely on automatic invalidation:

```bash
rm -f out.hyc
cargo run -- examples/fib.hy
```

The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, **older than any source file recorded in the archive** (entry *and* imported modules), or was built for a **different entry** than the one you are running (the shared `out.hyc` path is not per-file). The dedicated `compile` command always recompiles; `run` never recompiles (it rejects a version-mismatched archive and asks you to rebuild from source).

If worker/`use` modules change and prints or behavior look “stuck” or intermittent across runs, stale `out.hyc` is a common false lead — `join` always waits for the worker to finish.

## A simpler hello-world

For a minimal smoke test:

```bash
cargo run -- examples/print_literal.hy
```

Source:

```coil
fn main() {
    print "hello";
}
```

**Expected output:** `hello`

## Project layout

```
coil/
├── common/              # Opcodes, values, diagnostics, archive envelope
├── parser/              # Lexer + Pratt parser → AST
├── compiler/
│   ├── src/typechecking/  # Hindley–Milner inference
│   ├── src/pipeline.rs    # Compile driver, multi-file discovery
│   ├── src/il/            # Compile-time stack IL + label-safe lower/fuse
│   ├── src/peephole.rs    # Legacy fusion helpers (superseded by il::lower)
│   └── tests/             # Golden pipeline and diagnostic tests
├── machine/
│   ├── src/vm.rs          # Bytecode interpreter
│   ├── src/memory/        # Stack, heap, GC
│   └── src/ffi/           # libffi dynamic calls + host natives
├── examples/              # Runnable .hy programs (catalog in examples.md)
├── docs/
│   ├── manual/            # Getting started, tutorials, examples catalog (you are here)
│   ├── references/        # Language + per-API lookup
│   └── internals/         # Pipeline, VM notes, grammar
├── src/main.rs            # `cargo run` entry point
└── coil.toml.example      # Sample project manifest for modules
```

### Crate responsibilities

| Crate | Role |
|-------|------|
| `parser` | Turn `.hy` text into an AST (`Expression`, `Pattern`, declarations) |
| `compiler` | Typecheck, emit stack IL, lower/fuse to bytecode, write `.hyc` archives |
| `machine` | Execute bytecode; manage stack, heap, and automatic GC |
| `common` | Shared `Instruction` opcodes, `Value` representation, `ArchivedProgram` |

## Compilation model

coil uses **AST → stack IL (symbolic labels) → lower/fuse → bytecode**. There is no separate register-IR stage in the current tree.

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────┐
│  .hy source │ →  │ Parser (AST) │ →  │ HM checker  │ →  │ Stack IL │
└─────────────┘    └──────────────┘    └─────────────┘    └────┬─────┘
                                                                │
                    ┌──────────────┐    ┌─────────────┐         ▼
                    │ VM execute   │ ←  │  out.hyc    │ ←  Lower/fuse
                    └──────────────┘    │  (rkyv)     │
                                        └─────────────┘
```

### Stages in detail

1. **Parse** — `parser::Pratt` builds an AST. Syntax errors are reported with spans.
2. **Typecheck** — `compiler::typechecking::Checker` runs Algorithm W, producing a type for every expression and collecting diagnostics (unknown identifiers, unify errors, non-exhaustive `match`, and so on).
3. **Codegen (IL)** — walks the AST into a stack IL (`compiler/src/il`) with symbolic jump labels. A compile-time **`ConstEnv`** folds scalar `const` values, constant `if`/`while` conditions, and small constant-bound loops (unroll ≤ 8 trips). Direct tail-recursive `return f(...)` emits **`TailCall`**; tiny callees may be inlined at call sites.
4. **Lower** — after link, `finalize_bytecode` runs IL opts then fuse-select (`BinSlotImm`, `CmpJmpf`, …), assigns PCs once, and emits `Vec<Byte>`. Label binds act as fusion barriers.
5. **Archive** — bytecode and a constant pool are wrapped in `ArchivedProgram { version, bytecode, constants }` and serialized with rkyv. `ARCHIVE_VERSION` (currently **30**) must match at load time.
6. **Run** — `Machine::run_raw` deserializes and dispatches opcodes. Heap allocations trigger periodic mark-and-sweep GC.

### Entry point convention

Every program must define `fn main()`. The compiler emits a short prologue (`CALL`, `JMP`, `HALT`) and patches the `JMP` to jump to `main` (or to extern-block setup when `extern` declarations are present).

## Source and archive files

| Extension | Meaning |
|-----------|---------|
| `.hy` | Coil source (`.hy` = **henry**, the SI unit of inductance) |
| `.hyc` | Compiled bytecode archive (rkyv-serialized `ArchivedProgram`) |

The default CLI writes `out.hyc` in the working directory. Treat archives as **compiler-version-specific** — stale archives are rejected when `ARCHIVE_VERSION` changes.

## What you can write today

The language includes:

- **Primitives:** `int`, `float`, `string`, `bool`
- **Functions** with typed parameters and return types
- **`let` bindings** and reassignment (`x = expr;`)
- **Control flow:** `if` / `else`, `while` loops
- **Enums** with unit, tuple, and record-shaped variants
- **`match`** with constructors, wildcards, and nested record patterns
- **Tuples** `(a, b)`, **arrays** `[T]` / `[T; N]`, **dicts** `{ key: value }`
- **Type aliases** `type Point = (int, int);`
- **Modules** via `use foo::bar;` and `mod foo;` (multi-file projects; see [Modules](../references/modules.md))
- **FFI** via `extern "lib" { ... }` or runtime `dload` / `declare` / `invoke`
- **Classes** (partial — see `examples/classes.hy`)
- **Coroutines** — `async fn`, `yield`, `resume`, `resume h with v`, `let x = yield e`, `yield from` (see [tutorial/08-coroutines.md](tutorial/08-coroutines.md))

Not yet available: string concatenation with `+`, and a user-facing `format` keyword (use `print "%i", value` instead).

## Next steps

1. **Tutorial** — start with [01 — Basics](tutorial/01-basics.md) for a guided tour of syntax and types.
2. **Examples** — browse the full catalog in [examples.md](examples.md); each entry includes the run command and expected output.
3. **References** — keep [syntax](../references/syntax.md) and [types](../references/types.md) open while you code.
4. **Modules** — copy `coil.toml.example` to `coil.toml` when you split code across files; see [project config](../references/project-config.md).

### Suggested learning path

| Step | Example | Teaches |
|------|---------|---------|
| 1 | `print_literal.hy` | `print`, `main` |
| 2 | `let_test.hy` | `let`, reassignment |
| 3 | `fizbuz.hy` | `if`, modulo, multiple prints |
| 4 | `option.hy` | enums, `match` |
| 5 | `record.hy` | record variants, field access |
| 6 | `dict.hy` | anonymous records |
| 7 | `aliases.hy` | type aliases, tuples |
| 8 | `strlen.hy` or `ffi_sum.hy` | FFI (after installing libffi) |

Happy scripting.
