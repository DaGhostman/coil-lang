# coil — AGENTS

coil: statically typed `.hy` → stack IL → `.hyc` archive → custom VM.

| Need | Read |
|------|------|
| Write / edit `.hy` | `.cursor/skills/coil-language` · `docs/manual/` · `docs/references/` |
| Compiler, VM, pipeline | `.cursor/skills/coil-contributor` · `docs/internals/` |
| Hangs, panics, breakpoints | `.cursor/skills/coil-debug` · `docs/internals/debugger.md` |

## User preferences

- Run tests with 64MB limit: `ulimit -v 65536 && cargo run -- test` (leak smoke).
- Soft CPU baseline: `./scripts/poop_baseline.sh` — not a hard CI gate.
- Large tasks: scoped sub-agents on disjoint modules; avoid over-parallelizing.
- VM perf: prefer alloc reduction, hot-loop tuning, bounds-check elimination, `promise!` over new opcodes or IL opts; reject benchmark-shaped opcodes unless universal.
- Language features: draft plans first; full HM integration; update `docs/`; minimal runnable example with expected output.
- Granular conventional commits; stage only related files.
- Prefer compiler virtual modules over userland for core machinery.

## Invariants (do not break)

- **Append-only opcodes** in `common/src/opcode.rs` (`#[repr(u8)]` discriminants). New variants at the end only — then bump archive **minor** (`ARCHIVE_MINOR` in `common/src/archive.rs`), `promise!` ceiling in `machine/src/vm.rs`, and `instruction_from_u8_covers_last_appended_variant`. Incompatible ABI/layout changes bump **major** (reset minor). Load check: same major and archive minor ≤ runtime minor.
- **Virtual-module natives** via `HostInvoke` — not new opcodes for `io` / `thread` / etc.
- **Lint gate**: `cargo check --workspace` (not clippy — pre-existing `Gc::payload_mut` deny).

Codegen / IL / match / `STORE` rules and typechecker limitations: `.cursor/skills/coil-contributor/reference.md`. Pipeline stages: `docs/internals/pipeline.md`.

## Cloud agents

Pre-installed: `poop`, `valgrind`, `heaptrack`, `hyperfine`, `lua` (`.cursor/Dockerfile`). Use `--release` for benchmarks/`poop`.
