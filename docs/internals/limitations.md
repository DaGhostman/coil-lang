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

**Conservative copy propagation.** The IL pass forwards pure `CONST` / `CONST_POOL` / `STRING` / `LOAD` / slot-bin producers through straight-line `<producer>; STORE t; LOAD t` regions. It invalidates bindings at dependent stores, calls, memory operations, labels, and jumps. It only removes the original producer/store when `il::tell` proves the store's cursor floor is redundant; otherwise the store remains to protect shared local/operand slots. Aggregate materialization and field-key loads stay intact so specialized lowering can still form packed arrays and field-key CSE.

This is not general CFG copy propagation: joins, loops, unknown cursor states, calls, residual bytecode, and aliasing-sensitive operations remain fail-closed. **`il::sp` still is not a cursor model**: it tracks operand height, while `STORE` raises `tell` to `slot + 1` independently of height, and `Entry{Call}` has a special return-height rule. A broader pass would need relative cursor/liveness analysis rather than slot-index use counts.

Cursor rules established from the VM handlers: pushes/pops move it by ±n; `STORE` (packed `n`) pops `n` then raises it to `max(tell, max_written_slot + 1)`; `Seek s` sets it to `s`; `CALL` sets the callee frame base to `tell - arity`, and the matching return seeks back to that base and pushes the result, so the caller-relative effect is `-arity + 1`. The shared bytecode/symbolic-IL model is validated differentially against the VM; any future widening of copy propagation should preserve that gate because a cursor mistake is silent memory corruption, not a failing test.

Only the *bytecode* half of that model is under the differential gate, and the gap has already cost one bug: `effect_il` gave `Entry{Call}` a delta of `arity - 1` (the correct rule for `JumpIfMatch`) instead of `1 - arity`, so every symbolic-IL cursor past a call was wrong. Extending `cursor_model.rs` to diff the symbolic-IL path as well would close it.

**Slot promotion only moves what the cursor proves.** `opt::slot_promote` drops a `STORE t` reached with the cursor at `t + 1` (TOS already *is* slot `t`, so the write and the store's own floor are both no-ops) together with the reload run in front of a `TailCall` whose arguments are already the top `arity` stack positions. It fires only when every surviving reference to the slot is dropped with it, because eliding the store leaves the slot defined by a bare push that no slot-tracking pass can see — hence it also runs *after* per-body GVN. Deliberately refused:

| Refused | Why |
|---------|-----|
| Store to a slot nothing reads | Dead code, not promotion — `dead_store`'s cursor proof owns that call |
| Reload run in front of `CALL` | The callee frame base is `tell - arity`; a lower `tell` moves it down over caller slots, which needs slot liveness |
| `LOAD t; RETURN` (cursor-provable) | Measured net loss: it is the suffix the whole-buffer return convoys sink into a join, and eliding it in one predecessor kills the sink |
| Coalescing a copy whose destination is read in between | `mandelbrot`'s `tr → zr` needs the def sunk past the `zi` read — scheduling, not promotion |
| Anything inside a loop whose body raises the cursor | The header cursor is genuinely `Unknown` (see `il::tell`), so no store in the body is provably redundant. This is what still holds `mandelbrot`'s 13 STOREs: normalizing the back edge with a `Seek` would make the header `Known` and turn all three inner temps into self-stores, at one dispatch per iteration |
| Pool-packed fused slot operands (`BinSlot*Store` / `BinSlot*Jmpf`), `Seek`, `UnpackAt` | Destination slot is not readable from symbolic IL — the whole body is refused |

**Counted-loop bounds analysis proves length invariance, not in-bounds indices.** `il::bounds` answers one question per natural loop: can the length of the arrays this loop addresses change while it runs? Element writes cannot — `StoreIndex` overwrites a slot in place — so `while i < len(a) { a[i] = 0; }` has an invariant `len(a)` even though the array is mutated. On that proof two invariant materializations move to the preheader: the `LOAD a; ArrayLen; STORE t` triple codegen leaves in the loop header, and the `CONST imm; STORE t` pair that materializes a constant addressing operand (`vec_scan` 6.58M → 5.01M dispatches, `nsieve` 545.6k → 469.9k). **No bounds check is removed**: `Index` / `StoreIndex` keep the in-VM range test, so an out-of-range read still yields `-1` and an out-of-range write is still a no-op.

The safety argument is the cursor, not liveness: the preheader `STORE t` floors the cursor at `t + 1`, and because the cursor is monotone in its input, proving every in-loop stack height stays at or above the header's proves every in-loop push lands above `t`. That is why the pass needs only `il::sp`, and why it works where `slot_promote` cannot — it *adds* a floor instead of removing one. Deliberately refused:

| Refused | Why |
|---------|-----|
| Any call, host native, `GetField`/`SetField`, or unmodelled op in the body | The callee could hold another reference to the array and `push`/`pop` it. This is the single biggest coverage gap: most stdlib `while i < len(b)` loops call a helper on `b[i]` |
| `ArrayPush` / `MakeArray` / `MakeDict` / `CodePtr` / `MakePolyFn` in the body | Length can change (`tests/positive/while_len_grow.hy`) or user code can run |
| A rebound `Vec` local (`slots_stored_in_loop`) | A different array each pass, so its length is not invariant |
| An `Index` / `StoreIndex` whose target is not a plain slot load | Nested `a[i][j]`, a `Dup`, a call result: the walk-back cannot name the array, so the whole loop is refused |
| A loop that computes `len(a)` but addresses no array | Outside P2's remit; nothing licenses reasoning about aliasing there |
| A body whose stack height dips below the header's | The preheader floor would not survive, so a later push could land on the temp |
| A temp read before its def in the body, or outside the loop | The hoist changes what the earlier read observes; the cursor floor also stops protecting the slot once control leaves the loop |
| **`0 <= i < len` itself** | Implemented nowhere: with no unchecked opcode there is no consumer for the fact. Induction-variable detection plus a monotonicity proof is the next slice, and it only pays off together with an `IndexUnchecked`-style form or an in-VM object-lookup cache — both opcode/ABI decisions |
| The `find_object_by_addr` lookup each `Index` still pays | Caching the resolved array across a loop means keeping a heap address live in IL across a GC point; the length is an `int`, which is why it hoists and the object does not |

**The caller-side predicate peel only pays when it spills nothing.** When a callee opens with a pure guard over its parameters and returns an immediate or a parameter from that arm, codegen evaluates the guard at the call site so base cases skip the frame. Arguments that compile to a single pure byte (one slot load, one constant) are re-materialized in both the guard and the argument prep instead of being stored to a temp, which drops one `STORE` plus one spill `LOAD` per argument and leaves the guard reading the caller's own locals (peel-heavy loop: 4.28G → 3.29G instructions, 189ms → 152ms). Anything longer than a byte still takes a temp, because the guard copy and the call copy would each pay for it.

That byte budget is the whole profitability margin, and it is what rules out peeling a *self*-recursive call. A frame in this VM costs about two dispatches — `CALL`, then a fused `LoadReturnSlot` / `ConstReturnImm` that returns the base-case value — while the callee's guard is usually one fused `BinSlotSlotJmpf`. A peeled site has to re-emit that guard unfused (the operands are now caller expressions, not callee slots), add a `JMP` over the base arm, and store the join value, so it costs more than the frame it avoids *and* non-base calls pay the guard twice. Measured on `tak`, where 54% of the 63,609 calls hit the base case immediately: peeling all three inner self-calls grew the body from 13 to 41 words and cost +73.5% VM instructions and +31.4% wall time. Deliberately refused:

| Refused | Why |
|---------|-----|
| Self-recursive call sites | Callee span is not recorded until its body is compiled; reading the in-progress body works, but the peel loses to the frame (above) |
| A base-case value that is not returned | The peel replaces the callee's `return`; a value that falls through to the join is a different result |
| An argument longer than one pure byte, in the guard | Re-materializing it duplicates real work; it keeps its spill slot |
| Any argument with a side effect | The guard reads some arguments before the others are evaluated, and the false path evaluates them again |
| Instance methods, rest params, coroutines, un-monomorphized generics | The peel replicates the callee ABI, and `CallIndirect` receivers are not covered |
| A call site that is not saturated | Partial application lowers to `MakeFn`, not `CALL` |

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
