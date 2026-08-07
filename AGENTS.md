# coil — AGENTS

coil: statically typed `.hy` → stack IL → `.hyc` archive → custom VM.

| Need | Read |
|------|------|
| Write / edit `.hy` | `.cursor/skills/coil-language` · `docs/manual/` · `docs/references/` |
| Compiler, VM, pipeline | `.cursor/skills/coil-contributor` · `docs/internals/` |
| Hangs, panics, breakpoints | `.cursor/skills/coil-debug` · `docs/internals/debugger.md` |
| Known gaps / workarounds | `docs/internals/limitations.md` |

## User preferences

- Tests: `cargo test --workspace --all-targets` (required gate; covers integration tests). Feature isolation: `--no-default-features --features <crypto|time|regex|tls>` or `--features <dissect|debugger>`. Leak smoke: `ulimit -v 65536 && cargo run -- test`. Soft CPU: `./scripts/poop_baseline.sh`.
- Large tasks: scoped sub-agents on disjoint modules.
- VM perf: alloc reduction, hot-loop tuning, bounds-check elimination, `promise!` — not benchmark-shaped opcodes unless universal.
- Language features: draft plans; full HM; update `docs/`; minimal runnable example.
- **Method-based APIs** — prefer inherent/`impl` methods over free functions for type-tied operations (stdlib, new language surface, codegen fixes). Free generic fns returning enums are fragile today; see `docs/internals/limitations.md`.
- Granular conventional commits; stage only related files.
- Prefer compiler virtual modules over userland for core machinery.
- `cargo build` builds `coil` + `coil-debug` / `coil-dissect` / `coil-fmt` / `coil-lsp` / `coil-embed` (`coil package` defaults to embed).
- IL inspection: `coil dissect` — no verbose debug-build dumps.
- `coil fmt`: preserve `//` and `///`; wrap long lines; trailing commas on multi-line lists.

## Invariants (do not break)

- **Append-only opcodes** (`common/src/opcode.rs`). New variants at end → bump archive **minor**, `promise!` in `machine/src/vm.rs`, `instruction_from_u8_covers_last_appended_variant`. ABI break → **major** (reset minor).
- **Virtual-module natives** via `HostInvoke` — host wiring in `machine/`.
- **Feature gates**: debugger `feature = "debugger"`; dissect `feature = "dissect"` on helper binaries, not default `coil`.
- **Lint gate**: `cargo check --workspace` (not clippy — `Gc::payload_mut` deny).

Codegen / match / `STORE`: `.cursor/skills/coil-contributor/reference.md`. Pipeline: `docs/internals/pipeline.md`.

## Cloud agents

Pre-installed: `poop`, `valgrind`, `heaptrack`, `hyperfine`, `lua` (`.cursor/Dockerfile`). Use `--release` for benchmarks.
