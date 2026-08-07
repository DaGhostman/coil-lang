# Known limitations and workarounds

Actionable gaps in the compiler, VM, and language surface. For opcode/archive rules see [AGENTS.md](../../AGENTS.md); for typechecker error codes see [error-codes.md](../references/error-codes.md).

**Design rule:** prefer **method-based APIs** (`impl` methods on classes) over free functions for type-tied operations — stdlib, new language features, and codegen fixes should default to methods. Virtual-module host primitives (`io::read`) remain free functions.

**Priority key:** blocking = correctness crash or major feature blocked; high = reliability or significant DX gap; medium = partial support; low = deferred polish.

## Parser / surface (low–medium)

| Issue | Detail |
|-------|--------|
| `coil.toml` `preludes` / `strict` | Not implemented — [project-config.md](../references/project-config.md) |
| `import` keyword | Not implemented (use `use`) |
| `case` as `match` alias | Not in grammar |
| Range `collect` | `collect(0..5)` deferred; non-numeric `Ord` ranges not iterable |
| Duplicate record fields | Not rejected at parse time (typechecker reports) |

## Userland footguns

| Issue | Detail |
|-------|--------|
| `Option` field moves | `match` moves fields out — copy links before nested `match` on the same field |
| Cross-module user classes | Userland class types not importable across modules (e.g. `HashSet` wrapper) |
| User trait calls | Dictionary passing; only ground builtin bounds get direct monomorphized opcodes |

## IL optimizations (low)

GVN has no SSA slot rename; effectful ops are barriers. Its Dup-CSE is re-expanded to a second `LOAD` before lower (`expand_dup_after_load`) because `Dup` hides a binop's operands from fuse-select — keep that in mind before adding new Dup rewrites. Per-function `multi_op_join_convoy` can mis-sink on JMPF diamonds — whole-buffer pass required. Fuse-select intentionally conservative during JMP migration.

**No general copy propagation.** There is no pass that forwards `<producer>; STORE t` into a later `LOAD t` and deletes the copy, because slot liveness cannot be approximated by slot index on this VM: locals and the operand stack share one buffer, `STORE` raises `tell` past the written slot precisely to protect locals, `pop` lowers it, and a `CALL` places the callee frame above `tell`. Deleting a store therefore lowers `tell` and can move a callee frame over slots that are still live — the same hazard `dce::mem_fwd` refuses on. Doing this properly needs real backward liveness over the shared stack, not an index-based use count. Codegen avoids emitting the redundant temps instead (see the indexed-write path in `emit_write_lvalue`); the remaining known case is a constant spilled across a `CALL`, e.g. `1 + f(x)` keeping `CONST 1; STORE t`.

**`*Jmpf` has no `*Jmpt` counterpart.** `CmpJmpf` / `BinSlotImmJmpf` / `BinSlotSlotJmpf` / `LogNotJmpf` exist but there are no jump-if-true forms, so `opt::cfg::invert_branch_over_jump` refuses to invert a guard whose condition would fuse — inverting would trade one fused dispatch for two. Only non-fusable guards (bool locals, call/field results) collapse to `JMPT`.

## Test / CI reliability (high–medium)

| Issue | Detail |
|-------|--------|
| Stack-margin heap corruption (fixed) | Root-caused two *sibling* giant recursive-descent functions with oversized inline `match` arms: `Checker::infer_inner` (~3400 lines, ~102 arms — `Call` alone ~1000 lines) and `Compiler::do_compile` (~4400 lines — `Call` ~1020 lines, `Match` ~590 lines). Rust sizes a function's stack frame for the union of every arm's locals, so *every* recursive call paid for the biggest arm even on trivial input; a large-frame overflow can jump the guard page into adjacent memory instead of faulting cleanly, which is what surfaced as `corrupted double-linked list` / SIGSEGV / stack-smashing on random *unrelated* tests (confirmed via core dumps: crashing thread names were arbitrary, e.g. `examples/typecl`, `fallthrough_boo`). Fixed by extracting all oversized arms into their own `#[inline(never)]` methods in both functions, plus a `catch_unwind`-based recursion-depth guard (`infer_depth` / `codegen_depth`, `ErrorCode::ExpressionNestingTooDeep`) in each as defense-in-depth against any future/adversarial deep nesting. Cut `attr_on_async_fn_rejected_at_compile_time`'s minimum stack requirement from ~2 MiB to ~1 MiB (bisected); `RUST_MIN_STACK` / `run_example`'s thread stack are back down to 8 MiB (`ulimit -s` default) instead of the 32 MiB stopgap. |
| Parallel thread-churn crash / hang (open, root cause identified) | After the stack-margin fix above, repeated `cargo test -p compiler --test pipeline` runs still crash or hang roughly 1/4-1/3 of the time, but now the crashing thread is consistently a `thread::spawn` example (`thread_join.hy`, `thread_mutex.hy`, `thread_channel.hy`, `thread_reply.hy`), not a random victim — a genuinely different, still-open bug. A symbolized core dump of one SIGSEGV showed the fault inside `core::sync::atomic::atomic_compare_exchange` from `std::sys::pal::unix::stack_overflow::imp::make_handler` (Rust's per-thread SIGSEGV/altstack setup, invoked on every `thread::spawn`) while dozens of other threads were concurrently joining (`pipeline::run_example`) or idling in `machine::reactor::worker_loop`. Likely a race under very high thread churn — the test harness spawns one OS thread per example (`run_example`) *and* a full reactor worker pool per `thread::spawn`-using example, so 40-70+ threads can be alive at once. `Reactor::shutdown()` (see the now-fixed leak below) reduces steady-state thread count but not this peak. Needs either a bounded thread pool in `run_example` (stop spawning one OS thread per test) or investigating the std/libc altstack interaction directly. Workaround: `cargo test -p compiler --test pipeline -- --test-threads=1` avoids it entirely; re-run otherwise. |
| Reactor immortal-thread leak (fixed) | `Reactor::shutdown` existed but was never called, so every `thread::spawn`'d coil program leaked `worker_cap` OS threads for the rest of the process — they hold their own `Arc<Reactor>` clone and poll forever, compounding thread counts across a `cargo test` run (a contributing factor to the crash above, though not the whole story). Fixed by calling `Reactor::shutdown()` (joins pool threads) at the end of `Machine::run_with_pool` once the root's `live_threads` registry drains. |
| Criterion vs `--all-targets` | Prefer `cargo test --workspace --lib --tests --bins` (CI and agent gate). `--all-targets` also builds `[[bench]]` targets; Criterion treats argv after `--` as its own CLI and aborts the suite. |
| Optional feature suites | Full `cargo test` under `--no-default-features` (or a single of `crypto`/`time`/`regex`/`tls`) fails or hangs on tests that need the other libs. CI compile-gates those with `cargo check --workspace --lib --tests --bins …`; keep full tests on the default stack (plus `dissect` / `debugger`). |
| `ulimit` leak-smoke wraps `cargo run` | `ulimit -v 65536 && cargo run --bin coil -- test` OOMs immediately (`memory allocation of N bytes failed`) — it's `cargo`'s own build-check machinery that exceeds 64MB, not the `coil test` binary. The compiled binary itself passes cleanly under the same cap (305/305). Invoke the built binary directly: `cargo build --bin coil && (ulimit -v 65536; ./target/debug/coil test)`. |

## Tracking

Most items have no inline `TODO`/`FIXME` — knowledge lives here and in [.cursor/skills/coil-contributor/reference.md](../../.cursor/skills/coil-contributor/reference.md). Update this file when closing a limitation.
