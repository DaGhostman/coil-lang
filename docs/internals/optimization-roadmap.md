# AOT and JIT optimization roadmap

This document turns the current performance measurements into an ordered
optimization plan. The interpreter remains the compatibility path; new
optimizations must preserve archive compatibility and the existing VM
semantics.

## Baseline

Run the repeatable matrix with:

```bash
./scripts/perf_matrix.sh
```

The script builds the release binary, checks each benchmark checksum, compares
the four cross-language benchmarks (plus restored `fib`) against Lua and Node, runs the Coil-only
examples, and writes raw `poop` output plus metadata under
`/tmp/coil_perf_matrix/`. Set `OUT_DIR` for another location, `DURATION_MS`
for longer samples, or `RUN_MASSIF=1` to collect optional Valgrind Massif files.

The 2026-08-08 release baseline used precompiled Coil archives and 6-second
`poop` comparisons:

| Benchmark | Coil | Lua | Node | Dominant signal |
|-----------|------|-----|------|-----------------|
| `mandelbrot` | 32.5 ms | 15.0 ms | 15.8 ms | 573M VM instructions; numeric loop dispatch |
| `tak` | 2.09 ms | 1.31 ms | 13.4 ms | recursive direct-call/frame overhead |
| `nsieve` | 2.72 ms | 1.00 ms | 14.2 ms | array mutation, indexing, and bounds/object checks |
| `binary_trees` | 12.9 ms | 9.24 ms | 15.2 ms | heap allocation and GC |

Post–float-fusion soft baseline (`./scripts/poop_baseline.sh`, 2026-08-10):
`mandelbrot` ~19.6 ms / 392M instructions, `tak` ~2.17 ms, `nsieve` ~2.73 ms,
`binary_trees` ~12.4 ms (still directional; re-run `perf_matrix.sh` for cross-lang).

Coil used about 5.9–7.4 MB RSS, Lua 2.7–3.2 MB, and Node 89–91 MB. These are
directional comparisons rather than language rankings: the ports have
different runtime startup, library, and allocation behavior.

The repository also has Coil-only `numeric`, `operators_loop`, and `match_sum`
benchmarks. Their current results are retained by the matrix, but they have no
Lua or Node ports.

## Recently landed (float AOT)

Source-ordered float work on the interpreter path (no FMA / reassociation):

- LICM: full invariant float expression chains (past intermediate height-1).
- `FloatChainStore`: up to three stages; `BinSlotSlot` stage0; const-pool operands.
- `BinSlotSlotConstJmpf`: float mag arith + pool compare + `JMPF`.
- `NEGF` unary float negate.
- Algebraic: exact `+0.0` / `+1.0` float identities; const-pool float binop fold.
- Codegen: `new Class(args).field` scalar replacement (no temp instance).

Next AOT priorities below remain the main gap vs Lua on `mandelbrot` /
`tak` / `nsieve` / `binary_trees`.

## AOT priorities

### 1. Local slot promotion and SSA-like values

Priority: highest.

The shared operand/local stack still makes repeated `LOAD` / `STORE` traffic
expensive. `gvn.rs` explicitly has no SSA slot rename, and the new
cursor-safe copy propagation in `opt/dce.rs` is intentionally straight-line.
The next pass should promote slots to virtual values within a function:

- build per-function definitions and uses over symbolic `IlOp` blocks;
- retain values across straight-line code and joins only when all incoming
  definitions agree;
- invalidate or flush at calls, host effects, field mutation, unknown bytes,
  and address-sensitive operations;
- lower virtual values back to `LOAD` / `STORE` at boundaries;
- use the existing `tell` model as the safety proof for cursor changes.

This should be measured first on `mandelbrot` and `tak`, where it can expose
more `BinSlot*` and fused branch opportunities without changing the archive
format.

### 2. Loop range and bounds analysis

Priority: high.

`Index` and `StoreIndex` currently perform runtime object lookup and signed
bounds checks in `machine/src/vm.rs`. Add a proof-only IL analysis for common
counted loops:

- identify an invariant array/tuple value and a loop index;
- prove `0 <= i < len` for the loop body;
- hoist invariant length/object-kind information where the alias rules allow;
- keep the checked instruction on unknown or mutation-sensitive paths.

Start with diagnostics and bytecode-shape counters before adding a new opcode.
This targets `nsieve` and avoids benchmark-specific assumptions.

### 3. Allocation and GC fast paths

Priority: high for heap-heavy code.

`MakeTuple` and `MakeArray` currently collect stack values into a temporary
`Vec<Value>` before allocating the managed object. Profile
`machine/src/memory/heap.rs` and the aggregate handlers before changing
ownership:

- add allocation and collection counters under `vm_profile`;
- measure temporary vector allocations and live bytes;
- evaluate direct payload construction or a small fixed-arity fast path;
- keep GC rooting correct before and after allocation;
- consider region/batch allocation only after object lifetime boundaries are
  explicit.

This is the most direct path for `binary_trees`; it is independent of a JIT.

### 4. Direct-call and closure specialization

Priority: medium. **Status: partial (B4 landed).**

Landed for monomorphic known targets:

- ground trait / instance method sites emit direct `CALL` instead of
  `CodePtr` + `CallIndirect` when the entry and arity are static;
- self-recursive predicate peels (provisional body spans) so nested `tak`
  calls skip base-case frames;
- existing tiny direct-call inlining / monomorphization unchanged.

Still use `CallIndirect` for PolyFn locals, dictionary `Index` targets, and
generic shared-body evidence that is not static at the call site.

### 5. Dispatch and trace fusion

Priority: medium to low until measured.

`Machine::execute` already uses outlined dispatch, unchecked stack access, and
typed/fused opcodes. Larger universal superinstructions or short trace fusion
should be considered only if they improve multiple benchmarks. Keep symbolic IL
and the single `il::lower` pass as the source of truth; do not add an opcode
for one benchmark shape.

## Cranelift JIT feasibility

The current VM is a good fallback runtime but not a direct native ABI:

- `Value` is an untagged machine word containing immediates or raw heap
  pointers;
- locals and operands share `Stack<Value>` with a mutable `tell` cursor;
- `Frame` stores bytecode IP and stack base, while calls can re-enter through
  FFI callbacks;
- `HostInvoke`, FFI, coroutines, `CallIndirect`, debugger stops, and GC all
  require runtime coordination.

Cranelift's `JITBuilder` / `JITModule` provide the required define, finalize,
and function-pointer lookup operations:

- [JITBuilder](https://docs.rs/cranelift-jit/latest/cranelift_jit/struct.JITBuilder.html)
- [JITModule](https://docs.rs/cranelift-jit/latest/cranelift_jit/struct.JITModule.html)

Keep this dependency optional in a new `coil-jit` crate or a `jit` feature.
It should not be part of the default compiler or VM build.

```mermaid
flowchart LR
  archive[".hyc bytecode"] --> counters["hot function / loop counters"]
  counters -->|cold| interpreter["existing VM"]
  counters -->|hot supported function| il["optimized IlFunc"]
  il --> clif["Cranelift IR"]
  clif --> native["native code cache"]
  native --> helpers["runtime helpers / fallback"]
  helpers --> interpreter
```

### Initial JIT tier

Compile only functions containing typed numeric operations, local loads/stores,
comparisons, symbolic branches, and returns. Exclude heap allocation, field
access, host/FFI calls, coroutines, indirect calls, and debugger sessions.
This gives a useful first tier without requiring GC stack maps or speculative
deoptimization.

Use an opaque runtime context rather than exposing `Stack<Value>` internals:

```text
JitEntry(context, frame_base, stack_cursor) -> JitExit
JitExit = { reason, value, resume_pc }
```

The native body may use virtual registers for supported locals and return a
value directly. Unsupported work returns `JitExit::Fallback`; the interpreter
continues from a known bytecode boundary. Native code must not retain a heap
pointer across a helper or allocation call in this tier.

### Hotness and installation

- Count function entries first; add loop back-edge counters only after function
  JIT compilation is stable.
- Use a configurable threshold and an opt-in `--jit` / feature flag.
- Key compiled code by archive identity, function entry, and JIT version.
- Keep a runtime side table from bytecode entry PC to either bytecode or native
  entry; do not change archived `CALL` operands.
- Disable JIT dispatch while debugger state is attached, or force a deopt
  boundary before every debugger-visible operation.

## Staged gates

1. **Baseline gate:** `perf_matrix.sh` produces metadata and raw results for
   every comparison; no benchmark is accepted without a correctness checksum.
2. **AOT gate:** an optimization must improve a target benchmark by at least
   5% wall time or 10% VM instructions without regressing any benchmark by
   more than 2%, and must pass the full language and cursor-differential
   suites.
3. **JIT prototype gate:** compile one pure numeric function, call it from the
   VM, and fall back to bytecode for one unsupported operation. Verify identical
   output, no archive/opcode changes, and code-cache cleanup.
4. **JIT promotion gate:** include compile latency, warm-up time, steady-state
   wall time, RSS, and fallback frequency. Promote only if `mandelbrot` or
   `tak` improves materially after warm-up costs; otherwise prioritize slot
   promotion and allocation work.

Required verification remains:

```bash
cargo check --workspace
cargo test --workspace --lib --tests --bins
./target/debug/coil test
./scripts/perf_matrix.sh
```
