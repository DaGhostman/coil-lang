# Contributing to coil

## Build and test

```bash
cargo check --workspace          # lint gate
cargo test --workspace           # full suite (~minutes)
ulimit -v 65536 && cargo run -- test   # leak smoke (64MB)
```

Release build and soft CPU baseline:

```bash
cargo build --release --workspace
./scripts/poop_baseline.sh
```

## Where to change things

| Area | Location |
|------|----------|
| Syntax | `parser/` |
| Types / diagnostics | `compiler/src/typechecking/` |
| Codegen / IL | `compiler/src/codegen/`, `compiler/src/il/` |
| VM / natives | `machine/` |
| Opcodes / archive | `common/` |

Routing detail: [.cursor/skills/coil-contributor/SKILL.md](.cursor/skills/coil-contributor/SKILL.md).

## Invariants (do not break)

See [AGENTS.md](AGENTS.md). Highlights:

- Append-only opcodes — new `Instruction` variants at the end only; bump archive **minor**, VM `promise!` ceiling, and `instruction_from_u8_covers_last_appended_variant`.
- Virtual-module natives via `HostInvoke` — not new opcodes for `io` / `thread` / etc.
- Language features need full HM integration, `docs/` updates, and a minimal runnable example.
- **Method-based APIs** — prefer `impl` methods on classes over free functions for type-tied operations (stdlib, new surface). See [limitations.md](docs/internals/limitations.md) for codegen gaps on free generic enum returns.

Known gaps and workarounds: [docs/internals/limitations.md](docs/internals/limitations.md).

## Documentation

| Tree | When to update |
|------|----------------|
| `docs/manual/` | Tutorials, getting started, examples catalog |
| `docs/references/` | Syntax, per-API pages, error codes |
| `docs/internals/` | Pipeline, VM, tooling |
| `stdlib/` (submodule) | `///` doc comments on public API in [coil-stdlib](https://github.com/ardax-corp/coil-stdlib) |

## Commits

Granular conventional commits; stage only related files.
