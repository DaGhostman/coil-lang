# Getting Started

This guide walks you through building zero-script, running your first program, and understanding how source becomes bytecode on the VM.

## Prerequisites

### Rust toolchain

zero-script is a Rust workspace. Install a recent stable Rust toolchain ([rustup](https://rustup.rs/)) and ensure `cargo` is on your `PATH`.

```bash
rustc --version
cargo --version
```

### libffi (optional, for FFI examples)

Examples that call C code (`examples/strlen.0s`, `examples/ffi_sum.0s`) require **libffi** at link time.

| Platform | Package |
|----------|---------|
| Arch Linux | `libffi` |
| Debian / Ubuntu | `libffi-dev` |
| Fedora | `libffi-devel` |

You can build and run all non-FFI examples without libffi.

## Build the project

Clone the repository and build the workspace from the root:

```bash
cd zero-script
cargo build --workspace
```

For optimized binaries:

```bash
cargo build --release --workspace
```

A successful build produces the `zero-script` binary (via `src/main.rs`) plus the `parser`, `compiler`, `machine`, and `common` crates as libraries.

## Run your first program

The canonical starter example computes the 10th Fibonacci number recursively
(fast enough for debug builds; the release CPU bench uses
`examples/fib_bench.0s` with `fib(32)`):

```0s
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
cargo run -- examples/fib.0s
```

**Expected output:** `55`

The default CLI invocation compiles `examples/fib.0s` to bytecode, serializes it into `out.c0s` in the current directory (if needed), then loads and executes it on the VM.

### CLI commands

| Invocation | Meaning |
|------------|---------|
| `zero-script <file.0s>` | Compile to `out.c0s` (cached) and run |
| `zero-script compile <file.0s> [-o path]` | Compile only; default output is `out.c0s` |
| `zero-script run <file.c0s>` | Execute a previously compiled archive |
| `zero-script test [path] [--fail-fast]` | Compile and run every `.0s` under `[path]` (default `./tests`) |

Examples:

```bash
# Compile only, custom archive path
cargo run -- compile examples/fib.0s -o /tmp/fib.c0s

# Run that archive
cargo run -- run /tmp/fib.c0s

# Project tests (default root ./tests)
cargo run -- test
cargo run -- test ./tests
cargo run -- test --fail-fast   # stop after the first failed case
```

Layout under `./tests`:

| Path | Meaning |
|------|---------|
| `tests/**/*.0s` (except below) | Must compile; each `test("…")` case must return `Ok` |
| `tests/compile_fail/**/*.0s` | Must **fail** to compile (negative syntax / type tests) |
| `tests/positive/`, `tests/negative_runtime/` | Organized positive and soft-failure runtime suites |

A test file can declare multiple cases without `fn main`:

```0s
test("addition works") {
    assert(1 + 1 == 2)?;
}

test("subtraction works") {
    assert(5 - 3 == 2, "arith")?;
}
```

Each `test("…") { … }` body runs in Result mode. A failed `assert`/`?` or a language `panic` fails that case; by default the harness continues to the next case (each case runs in an isolated VM so a panic does not skip later cases). Failures print `> Test "<description>" failed`. Files without `test(...)` cases still use a single `fn main()` as one opaque case. Files under `compile_fail/` pass when compilation rejects them (diagnostics are silenced so the summary stays readable).

### Recompiling after changes

The default (`BuildAndRun`) path caches `out.c0s`. After editing a `.0s` file, delete the archive to pick up changes, or rely on automatic invalidation:

```bash
rm -f out.c0s
cargo run -- examples/fib.0s
```

The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, or older than the entry source. The dedicated `compile` command always recompiles; `run` never recompiles (it rejects a version-mismatched archive and asks you to rebuild from source).

## A simpler hello-world

For a minimal smoke test:

```bash
cargo run -- examples/print_literal.0s
```

Source:

```0s
fn main() {
    print "hello";
}
```

**Expected output:** `hello`

## Project layout

```
zero-script/
├── common/              # Opcodes, values, diagnostics, archive envelope
├── parser/              # Lexer + Pratt parser → AST
├── compiler/
│   ├── src/typechecking/  # Hindley–Milner inference
│   ├── src/pipeline.rs    # Compile driver, multi-file discovery
│   ├── src/peephole.rs    # Opcode fusion pass
│   └── tests/             # Golden pipeline and diagnostic tests
├── machine/
│   ├── src/vm.rs          # Bytecode interpreter
│   ├── src/memory/        # Stack, heap, GC
│   └── src/ffi/           # libffi dynamic calls + host natives
├── examples/              # Runnable .0s programs (catalog in examples.md)
├── docs/                  # User documentation (you are here)
├── src/main.rs            # `cargo run` entry point
└── zero.toml.example      # Sample project manifest for modules
```

### Crate responsibilities

| Crate | Role |
|-------|------|
| `parser` | Turn `.0s` text into an AST (`Expression`, `Pattern`, declarations) |
| `compiler` | Typecheck, emit bytecode, peephole-optimize, write `.c0s` archives |
| `machine` | Execute bytecode; manage stack, heap, and automatic GC |
| `common` | Shared `Instruction` opcodes, `Value` representation, `ArchivedProgram` |

## Compilation model

zero-script uses a **single-pass stack codegen** pipeline (with a post-codegen peephole pass). There is no separate register-IR stage in the current tree.

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────┐
│  .0s source │ →  │ Parser (AST) │ →  │ HM checker  │ →  │ Codegen  │
└─────────────┘    └──────────────┘    └─────────────┘    └────┬─────┘
                                                                │
                    ┌──────────────┐    ┌─────────────┐         ▼
                    │ VM execute   │ ←  │  out.c0s    │ ←  Peephole
                    └──────────────┘    │  (rkyv)     │
                                        └─────────────┘
```

### Stages in detail

1. **Parse** — `parser::Pratt` builds an AST. Syntax errors are reported with spans.
2. **Typecheck** — `compiler::typechecking::Checker` runs Algorithm W, producing a type for every expression and collecting diagnostics (unknown identifiers, unify errors, non-exhaustive `match`, and so on).
3. **Codegen** — `Compiler::compile` walks the AST and appends stack instructions (`LOAD`, `CONST`, `JMP`, `MakeEnum`, `StorePop`, …) to a bytecode vector.
4. **Peephole** — `peephole::optimize` fuses frequent instruction sequences (`LOAD; CONST; ADD` → `BinSlotImm`, and similar) and relocates jump targets.
5. **Archive** — bytecode and a constant pool are wrapped in `ArchivedProgram { version, bytecode, constants }` and serialized with rkyv. `ARCHIVE_VERSION` (currently **9**) must match at load time.
6. **Run** — `Machine::run_raw` deserializes and dispatches opcodes. Heap allocations trigger periodic mark-and-sweep GC.

### Entry point convention

Every program must define `fn main()`. The compiler emits a short prologue (`CALL`, `JMP`, `HALT`) and patches the `JMP` to jump to `main` (or to extern-block setup when `extern` declarations are present).

## Source and archive files

| Extension | Meaning |
|-----------|---------|
| `.0s` | zero-script source |
| `.c0s` | Compiled bytecode archive (rkyv-serialized `ArchivedProgram`) |

The default CLI writes `out.c0s` in the working directory. Treat archives as **compiler-version-specific** — stale archives are rejected when `ARCHIVE_VERSION` changes.

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
- **Modules** via `use foo::bar;` and `mod foo;` (multi-file projects; see [reference/modules.md](reference/modules.md))
- **FFI** via `extern "lib" { ... }` or runtime `dload` / `declare` / `invoke`
- **Classes** (partial — see `examples/classes.0s`)
- **Coroutines** — `async fn`, `yield`, `resume`, `resume h with v`, `let x = yield e`, `yield from` (see [tutorial/08-coroutines.md](tutorial/08-coroutines.md))

Not yet available: string concatenation with `+`, and a user-facing `format` keyword (use `print "%i", value` instead).

## Next steps

1. **Tutorial** — start with [01 — Basics](tutorial/01-basics.md) for a guided tour of syntax and types.
2. **Examples** — browse the full catalog in [examples.md](examples.md); each entry includes the run command and expected output.
3. **Reference** — keep [reference/syntax.md](reference/syntax.md) and [reference/types.md](reference/types.md) open while you code.
4. **Modules** — copy `zero.toml.example` to `zero.toml` when you split code across files; see [reference/project-config.md](reference/project-config.md).

### Suggested learning path

| Step | Example | Teaches |
|------|---------|---------|
| 1 | `print_literal.0s` | `print`, `main` |
| 2 | `let_test.0s` | `let`, reassignment |
| 3 | `fizbuz.0s` | `if`, modulo, multiple prints |
| 4 | `option.0s` | enums, `match` |
| 5 | `record.0s` | record variants, field access |
| 6 | `dict.0s` | anonymous records |
| 7 | `aliases.0s` | type aliases, tuples |
| 8 | `strlen.0s` or `ffi_sum.0s` | FFI (after installing libffi) |

Happy scripting.
