# zero-script

**zero-script** is a statically typed scripting language with Hindley–Milner type inference. Programs are compiled to bytecode and executed on a custom virtual machine. Source files use the `.0s` extension; compiled archives are stored as `.c0s` files.

The language targets embeddable scripting: you get real type checking and inference without a heavyweight build pipeline, plus optional FFI for calling into C libraries or host-provided Rust closures.

## Quick start

```0s
fn main() {
    print "Hello, world!";
}
```

Run any program from the repository root:

```bash
cargo build --workspace
cargo run -- examples/print_literal.0s
```

Expected output: `hello`

See [Getting Started](getting-started.md) for prerequisites, project layout, and a guided first run.

## How programs run

1. **Parse** — the Pratt parser reads `.0s` source into an AST.
2. **Typecheck** — Algorithm W (Hindley–Milner) infers types and reports source-anchored diagnostics.
3. **Codegen** — the compiler emits stack bytecode, then runs a peephole fusion pass.
4. **Archive** — bytecode is wrapped in a versioned `ArchivedProgram` envelope (`ARCHIVE_VERSION` is currently **15**) and written to `out.c0s` on first run.
5. **Execute** — the VM loads the archive and runs `main`.

Re-run the same binary without deleting `out.c0s` to reuse the cached compile. Delete `out.c0s` (or bump the archive version) to force a fresh compile. The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, or older than the entry source.

## Language at a glance

| Area | Status |
|------|--------|
| Primitives | `int`, `float`, `string`, `bool`, `byte` |
| Functions, `let` / `const`, `if`/`else`, `while` / `for` | Supported |
| `break` / `continue` | Supported |
| Enums, `match`, record variants | Supported |
| Built-in `Option` / `Result`, `raise`, `?`, `??`, `?.` | Supported (desugar to match/return) |
| Tuples, arrays (`push` / `len`), dicts (anonymous records) | Supported |
| Type aliases (`type Name = T;`, lexically scoped) | Supported |
| Generics and traits | Supported: generic functions/enums/aliases/classes, higher-kinded type parameters, associated types/GATs, existentials, coherence checks |
| Modules / namespaces (`use`, `mod`) | Supported (multi-file CLI via `zero.toml`) |
| Field access (`p.x`, chained `p.x.y`) | Supported |
| FFI (`extern` blocks, `dload`/`declare`/`invoke`, struct/callback returns) | Supported (requires libffi) |
| IO streams (`use io::*;`, `[byte]`, files, sync adapters, TCP, UDP) | Supported (non-blocking L0; no HTTP in VM) |
| Classes (`class` / `impl` / `new`, fields, methods) | Supported |
| Coroutines (`async`, `yield`, `resume`, `yield from`, `done`) | Supported |
| `for x in` (Iterator / IntoIterator) | Supported (arrays, homogeneous tuples/dicts, coroutines, user `impl`s) |
| String concat via `+` | Supported (`string + string` → `string`) |
| `format` keyword | Supported (returns `string`; same specifiers as `print`) |

Browse runnable demos in [Examples](examples.md).

## Documentation

### New to zero-script?

Work through the tutorial in order. Each chapter builds on the previous one.

| Chapter | Topic |
|---------|-------|
| [01 — Basics](tutorial/01-basics.md) | Syntax, functions, `let`, control flow |
| [02 — Types & Variables](tutorial/02-types-and-variables.md) | Primitives, inference, annotations |
| [03 — Enums & Match](tutorial/03-enums-and-match.md) | Sum types and pattern matching |
| [04 — Records & Fields](tutorial/04-records-and-fields.md) | Record variants, field access, nested patterns |
| [05 — Aggregates](tutorial/05-aggregates.md) | Tuples, arrays, dicts, type aliases |
| [06 — Modules](tutorial/06-modules.md) | `use`, `mod`, `zero.toml` |
| [07 — FFI](tutorial/07-ffi.md) | `extern` blocks and dynamic loading |
| [08 — Coroutines](tutorial/08-coroutines.md) | `async fn`, resume, send/receive, `yield from`, `for x in` |
| [09 — Error handling](tutorial/09-error-handling.md) | Built-in Option/Result, `raise`, `?`, `??`, `?.` |
| [10 — IO streams](tutorial/10-io-streams.md) | `byte` / `[byte]`, `Stream`, files, sync adapters, TCP |

Classes (`class`, `impl`, `new`, field access, methods) are supported — see [02 — Types & Variables](tutorial/02-types-and-variables.md) and `examples/classes.0s`.

Start here: [Getting Started](getting-started.md)

### Reference

Look up syntax and semantics when you already know what you need.

| Document | Contents |
|----------|----------|
| [Syntax](reference/syntax.md) | Grammar overview, declarations, expressions |
| [Types](reference/types.md) | Type system, aliases, aggregates |
| [Operators](reference/operators.md) | Arithmetic, comparison, logical, field access |
| [Keywords](reference/keywords.md) | Reserved words and constructs |
| [Built-ins](reference/built-ins.md) | `print`, `format`, FFI builtins, natives |
| [Modules](reference/modules.md) | Namespace rules, `use` resolution |
| [Project config](reference/project-config.md) | `zero.toml` manifest format |
| [Error codes](reference/error-codes.md) | Stable `E####` diagnostic codes, SARIF / LSP flags |

### Examples catalog

| Document | Contents |
|----------|----------|
| [Examples](examples.md) | Every file in `examples/`, grouped by topic, with expected output |

## Repository layout

```
zero-script/
├── common/          # Shared types: opcodes, values, archive format
├── parser/          # Pratt parser and AST
├── compiler/        # HM typechecker, codegen, pipeline, peephole
├── machine/         # VM, heap/GC, FFI (libffi)
├── examples/        # Runnable .0s demos (see examples.md)
├── docs/            # This documentation
├── src/main.rs      # CLI: default build+run, compile, run, test
└── zero.toml.example  # Example project manifest
```

## Building and running

```bash
# Build everything
cargo build --workspace

# Default: compile to out.c0s (cached) and run
cargo run -- examples/fib.0s

# Compile only / run archive / project tests
cargo run -- compile examples/fib.0s -o fib.c0s
cargo run -- run fib.c0s
cargo run -- test   # every .0s under ./tests

# Release build
cargo build --release --workspace
cargo run --release -- examples/fib.0s
```

| Command | Role |
|---------|------|
| *(no subcommand)* `<file.0s>` | Compile → `out.c0s` (cached) → run |
| `compile <file.0s> [-o path]` | Compile entry file to a `.c0s` archive |
| `run <file.c0s>` | Execute a compiled archive |
| `test` | Compile+run all `./tests/**/*.0s` |

For FFI examples you also need **libffi** (e.g. `libffi-dev` on Debian/Ubuntu, `libffi` on Arch). See [Getting Started](getting-started.md).

## Learn by example

| Goal | Start with |
|------|------------|
| First program | [getting-started.md](getting-started.md) → `examples/fib.0s` |
| Enums & pattern matching | `examples/option.0s`, `examples/result.0s` |
| Record-shaped variants | `examples/record.0s`, `examples/mixed.0s` |
| Dicts / anonymous records | `examples/dict.0s` |
| Generics & traits | `examples/generics.0s`, `examples/hkt_bifunctor.0s`, `examples/gat_pointer.0s`, `examples/existential_show.0s` |
| Modules | `examples/modules.0s` (see [examples.md](examples.md) for setup) |
| FFI | `examples/strlen.0s`, `examples/ffi_sum.0s` |
| IO streams | `examples/io_bytes.0s`, `examples/io_file.0s`, `examples/io_eof.0s`, `examples/io_udp.0s` |
| Coroutines | `examples/coro.0s`, `examples/coro_gen.0s`, `examples/coro_send.0s`, `examples/for_in_coro.0s` |
| Full catalog | [examples.md](examples.md) |
