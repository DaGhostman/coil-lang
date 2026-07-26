# coil

**coil** is a statically typed scripting language with Hindley–Milner type inference. Programs are compiled to bytecode and executed on a custom virtual machine. Source files use the `.hy` extension (short for **henry**, the SI unit of inductance — the measure of a coil); compiled archives are stored as `.hyc` files.

The language targets embeddable scripting: you get real type checking and inference without a heavyweight build pipeline, plus optional FFI for calling into C libraries or host-provided Rust closures.

## Quick start

```coil
fn main() {
    print "Hello, world!";
}
```

Run any program from the repository root:

```bash
cargo build --workspace
cargo run -- examples/print_literal.hy
```

Expected output: `hello`

See [Getting Started](getting-started.md) for prerequisites, project layout, and a guided first run.

## How programs run

1. **Parse** — the Pratt parser reads `.hy` source into an AST.
2. **Typecheck** — Algorithm W (Hindley–Milner) infers types and reports source-anchored diagnostics.
3. **Codegen** — the compiler emits stack bytecode, then runs a peephole fusion pass.
4. **Archive** — bytecode is wrapped in a versioned `ArchivedProgram` envelope (`ARCHIVE_VERSION` is currently **28**) and written to `out.hyc` on first run. See [Debug line table](reference/debug-info.md).
5. **Execute** — the VM loads the archive and runs `main`.

Re-run the same binary without deleting `out.hyc` to reuse the cached compile. Delete `out.hyc` (or bump the archive version) to force a fresh compile. The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, or older than the entry source.

## Language at a glance

| Area | Status |
|------|--------|
| Primitives | `int`, `float`, `string`, `bool`, `byte` |
| Functions, `let` / `const`, `if`/`else`, `while` / `for` | Supported |
| Named call-site arguments (`f(name: v)`) | Supported (positional prefix then named; named holes on partials allowed) |
| Arity overloads / first-class fn values / lambdas (`use`) | Supported (`examples/overload.hy`, `fn_value.hy`, `lambda.hy`) |
| Rest parameters (`T... xs` / tuple `... xs`) | Supported (trailing only; `T...` packs to `[T]`, bare `...` packs to a tuple) |
| Call-site spread (`f(...pack)`) | Supported (tuple and array operands) |
| User-defined `attr` decorators | Supported (`attr` decl + `#[name(...)]` on `fn`, methods, class constructors) |
| Let destructuring (`let (a, b) = …`, `let { x, y } = …`) | Supported (tuple / record; no enum ctor patterns in `let`) |
| `break` / `continue` | Supported |
| Enums, `match`, record variants | Supported |
| Built-in `Option` / `Result`, `raise`, `?`, `??`, `?.` | Supported (desugar to match/return) |
| Tuples, arrays (`arr[] =` / `len`), dicts (anonymous records) | Supported |
| Type aliases (`type Name = T;`, lexically scoped) | Supported |
| Generics and traits | Supported: generic functions/enums/aliases/classes, higher-kinded type parameters, associated types/GATs, existentials, coherence checks |
| Modules / namespaces (`use`, `mod`) | Supported (multi-file CLI via `coil.toml`) |
| Field access (`p.x`, chained `p.x.y`) | Supported |
| FFI (`extern` blocks, `dload`/`declare`/`invoke`, C varargs `...`, struct/callback returns) | Supported (requires libffi) |
| IO streams (`use io::*;`, `[byte]`, files, sync adapters, TCP, UDP) | Supported (non-blocking L0; no HTTP in VM) |
| Classes (`class` / `impl` / `new`, fields, methods) | Supported |
| Coroutines (`async`, `yield`, `resume`, `yield from`, `done`) | Supported |
| `for x in` (Iterator / IntoIterator) | Supported (arrays, homogeneous tuples/dicts, ranges, coroutines, user `impl`s) |
| Ranges (`a..b` / `a..=b`) | Supported — lazy `Range<T: Ord>`; `for` steps `int`/`byte`/`float`; no auto array materialize ([syntax](reference/syntax.md#ranges-lazy)) |
| String concat via `+` | Supported (`string + string` → `string`) |
| `format` keyword | Supported (returns `string`; same specifiers as `print`) |

Browse runnable demos in [Examples](examples.md). Multi-file showcase apps (todo board, text adventure, TCP echo) live under [`examples/projects/`](../examples/projects/README.md).

## Documentation

### New to coil?

Work through the tutorial in order. Each chapter builds on the previous one.

| Chapter | Topic |
|---------|-------|
| [01 — Basics](tutorial/01-basics.md) | Syntax, functions, `let`, control flow |
| [02 — Types & Variables](tutorial/02-types-and-variables.md) | Primitives, inference, annotations |
| [03 — Enums & Match](tutorial/03-enums-and-match.md) | Sum types and pattern matching |
| [04 — Records & Fields](tutorial/04-records-and-fields.md) | Record variants, field access, nested patterns |
| [05 — Aggregates](tutorial/05-aggregates.md) | Tuples, arrays, dicts, type aliases |
| [06 — Modules](tutorial/06-modules.md) | `use`, `mod`, `coil.toml` |
| [07 — FFI](tutorial/07-ffi.md) | `extern` blocks and dynamic loading |
| [08 — Coroutines](tutorial/08-coroutines.md) | `async fn`, resume, send/receive, `yield from`, `for x in` |
| [09 — Error handling](tutorial/09-error-handling.md) | Built-in Option/Result, `raise`, `?`, `??`, `?.` |
| [10 — IO streams](tutorial/10-io-streams.md) | `byte` / `[byte]`, `Stream`, files, sync adapters, TCP |

Classes (`class`, `impl`, `new`, field access, methods) are supported — see [02 — Types & Variables](tutorial/02-types-and-variables.md) and `examples/classes.hy`.

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
| [Project config](reference/project-config.md) | `coil.toml` manifest format |
| [Error codes](reference/error-codes.md) | Stable `E####` diagnostic codes, SARIF / LSP flags |

### Examples catalog

| Document | Contents |
|----------|----------|
| [Examples](examples.md) | Every file in `examples/`, grouped by topic, with expected output |
| [Showcase projects](../examples/projects/README.md) | Multi-file apps (`01-todo`, `02-adventure`, `03-echo`) + co-located tests |

## Repository layout

```
coil/
├── common/          # Shared types: opcodes, values, archive format
├── parser/          # Pratt parser and AST
├── compiler/        # HM typechecker, codegen, pipeline, peephole
├── machine/         # VM, heap/GC, FFI (libffi)
├── examples/        # Runnable .hy demos (see examples.md)
│   └── projects/    # Showcase multi-file apps + co-located tests
├── docs/            # This documentation
├── src/main.rs      # CLI: default build+run, compile, run, test
└── coil.toml.example  # Example project manifest
```

## Building and running

```bash
# Build everything
cargo build --workspace

# Default: compile to out.hyc (cached) and run
cargo run -- examples/fib.hy

# Compile only / run archive / project tests
cargo run -- compile examples/fib.hy -o fib.hyc
cargo run -- run fib.hyc
cargo run -- test   # every .hy under ./tests

# Release build
cargo build --release --workspace
cargo run --release -- examples/fib.hy
```

| Command | Role |
|---------|------|
| *(no subcommand)* `<file.hy>` | Compile → `out.hyc` (cached) → run |
| `compile <file.hy> [-o path]` | Compile entry file to a `.hyc` archive |
| `run <file.hyc>` | Execute a compiled archive |
| `package <file.hy> [-o path] [--check-native]` | Single executable for this OS/arch (embedded `.hyc`) |
| `test [path] [--fail-fast]` | Compile+run all `[path]/**/*.hy` (default `./tests`); continue after failures unless `--fail-fast` |

For FFI examples you also need **libffi** (e.g. `libffi-dev` on Debian/Ubuntu, `libffi` on Arch). See [Getting Started](getting-started.md).

## Learn by example

| Goal | Start with |
|------|------------|
| First program | [getting-started.md](getting-started.md) → `examples/fib.hy` |
| Enums & pattern matching | `examples/option.hy`, `examples/result.hy` |
| Record-shaped variants | `examples/record.hy`, `examples/mixed.hy` |
| Dicts / anonymous records | `examples/dict.hy` |
| Generics & traits | `examples/generics.hy`, `examples/hkt_bifunctor.hy`, `examples/gat_pointer.hy`, `examples/existential_show.hy` |
| Modules | `examples/modules.hy` (see [examples.md](examples.md) for setup) |
| FFI | `examples/strlen.hy`, `examples/ffi_sum.hy`, `examples/ffi_printf.hy` |
| IO streams | `examples/io_bytes.hy`, `examples/io_file.hy`, `examples/io_eof.hy`, `examples/io_udp.hy` |
| Coroutines | `examples/coro.hy`, `examples/coro_gen.hy`, `examples/coro_send.hy`, `examples/for_in_coro.hy` |
| Full catalog | [examples.md](examples.md) |
