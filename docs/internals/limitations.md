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

GVN has no SSA slot rename; effectful ops are barriers. Per-function `multi_op_join_convoy` can mis-sink on JMPF diamonds — whole-buffer pass required. Fuse-select intentionally conservative during JMP migration.

## Test / CI reliability (high–medium)

| Issue | Detail |
|-------|--------|
| Parallel heap corruption (reduced, not eliminated) | Root-caused two contributors: (1) `Reactor` pool threads never stopped — `shutdown` was written but never called, so every `thread::spawn`'d coil program leaked `worker_cap` OS threads for the rest of the process, compounding thread counts across a `cargo test` run; fixed by calling `Reactor::shutdown()` (joins pool threads) at the end of `Machine::run_with_pool` once the root's `live_threads` registry drains. (2) Attr/coroutine-desugaring typechecker recursion (`Checker::infer`/`infer_inner`) uses unusually large stack frames — confirmed via a 512 KiB-vs-8 MiB thread-stack bisection that `attr_on_async_fn_rejected_at_compile_time` needs ~1.5-2 MiB just to typecheck a 12-line program. On a large frame this can jump straight past the guard page into an adjacent thread's stack or the heap instead of faulting cleanly, which is what surfaces as `corrupted double-linked list` / `*** stack smashing detected ***` / SIGSEGV on a *different, unrelated* test (confirmed via core dumps: crashing thread names were random victims like `examples/typecl`, `fallthrough_boo`, not an attr/thread test). Mitigated by raising `run_example`'s dedicated thread stack (4→32 MiB) and setting `RUST_MIN_STACK=32MiB` workspace-wide in `.cargo/config.toml` so every libtest per-test thread gets the same headroom. This cut repro frequency from ~2/3 of `cargo test --workspace` runs to roughly 1/8-1/3 under heavy parallel load (still not zero) — the underlying fix is reducing `infer_inner`'s per-frame stack footprint or bounding its recursion, not just adding headroom. Workaround: re-run; `cargo test -p compiler --test pipeline -- --test-threads=1` avoids it entirely. |
| Parallel `thread_join` / mutex hang | `example_thread_join_prints_42` and `example_thread_mutex_prints_2` pass alone (~tens of ms) but could hang indefinitely under default-parallelism `cargo test -p compiler --test pipeline`. The `Reactor` immortal-thread leak above (fixed) was a contributing factor; not reproduced in several dozen runs since the `Reactor::shutdown()` fix, but treat as reduced-not-proven-gone. Workaround: re-run, or `cargo test -p compiler --test pipeline -- --test-threads=1` / run the thread tests alone. |
| Criterion vs `--all-targets` | Prefer `cargo test --workspace --lib --tests --bins` (CI and agent gate). `--all-targets` also builds `[[bench]]` targets; Criterion treats argv after `--` as its own CLI and aborts the suite. |
| Optional feature suites | Full `cargo test` under `--no-default-features` (or a single of `crypto`/`time`/`regex`/`tls`) fails or hangs on tests that need the other libs. CI compile-gates those with `cargo check --workspace --lib --tests --bins …`; keep full tests on the default stack (plus `dissect` / `debugger`). |
| `ulimit` leak-smoke wraps `cargo run` | `ulimit -v 65536 && cargo run --bin coil -- test` OOMs immediately (`memory allocation of N bytes failed`) — it's `cargo`'s own build-check machinery that exceeds 64MB, not the `coil test` binary. The compiled binary itself passes cleanly under the same cap (305/305). Invoke the built binary directly: `cargo build --bin coil && (ulimit -v 65536; ./target/debug/coil test)`. |

## Tracking

Most items have no inline `TODO`/`FIXME` — knowledge lives here and in [.cursor/skills/coil-contributor/reference.md](../../.cursor/skills/coil-contributor/reference.md). Update this file when closing a limitation.
