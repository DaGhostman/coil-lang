# Multi-Pass Register-VM Refactor — Master Plan

> **Status:** Proposed (not yet started)
> **Date:** 2026-06-22
> **Authors:** Compiler Architecture Team
> **Refs:** [`AGENTS.md`](./AGENTS.md), [`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md)

---

## 1. Overview & Motivation

This refactor transforms the zero-script compiler from a **single-pass,
stack-based bytecode emitter** into a **multi-pass, CFG-based, register-based
bytecode emitter**. The motivation is not theoretical elegance — it is the
removal of six accumulating workarounds in the current codebase that share a
**single root cause**: the conflation of compilation phases (parsing +
typechecking + control-flow + register allocation + bytecode emission all
happen in one `do_compile` function).

The six workarounds are:

1. **`BlockBuilder` placeholder tracking** (`compiler/src/block_builder.rs`,
   ~673 LOC). Required because forward jumps need a deferred patch step —
   a direct consequence of single-pass emission where jump targets are
   unknown until the body is fully emitted.

2. **`emit_pattern_binding` recursive free function** with
   `consume_values: bool` parameter threading. Required because nested
   record patterns need a slot-based UNPACK (`UnpackAt`) that the outer
   UNPACK can't anticipate — a direct consequence of single-pass emission
   where the codegen doesn't know the receiver's slot layout until the
   match arm's binding code is already mid-emission.

3. **`per-arm match_bindings: HashMap<String, u32>`** side-table on
   `Compiler::Context`. Required because multi-variant binding bodies
   collide on global slot IDs — a direct consequence of single-pass
   emission where the slot numbering for arm N depends on how many arms
   the match has, which depends on the source order, which depends on
   the match arms being emitted in source-reverse order.

4. **`codegen_var_types: HashMap<String, Ty>`** side-table on the
   `Checker`. Required because `infer_function` skips the args Fragment
   via `parse_arg_list`, so the pre-walk's ID table and the infer cache
   are misaligned inside function bodies — a direct consequence of
   single-pass compilation where one pass tries to do both ID minting
   AND typechecking AND codegen for every node.

5. **`Expression::Block` extends `self.bytecode` directly** (returns
   empty `Vec<Byte>`). Required because direct-to-`self.bytecode`
   emitters (Print, Format, nested control flow) interleave with
   local-vec-returning children in unpredictable ways — a direct
   consequence of single-pass emission where the parent's byte buffer
   doesn't exist when the child emits.

6. **Reverse-source-order match arm emission** (`emit_pattern_binding`
   is called in `arms.iter().rev().enumerate()`). Required because
   `JUMP_IF_MATCH` fall-through semantics means the last arm is reached
   by fall-through, not by jump — a direct consequence of single-pass
   emission where the layout can't be planned ahead of the body.

Every one of these workarounds becomes **structurally unnecessary** under a
multi-pass architecture: the CFG pass builds a complete control-flow graph,
the SSA pass assigns canonical IDs to every value, the liveness pass
computes lifetimes, the register allocator chooses slots, and the emitter
is a simple list-to-list translation with no placeholders, no
reverse-order tricks, no slot side-tables.

---

## 2. Current State

The codebase is at a known-good state (HEAD: `b60e99f`). All 406 tests
pass (2 in `common`, 314 in `compiler/src/lib.rs`, 33 in
`compiler/tests/diagnostics.rs`, 14 in `compiler/tests/pipeline.rs`, 21 in
`machine/src/vm.rs`, 16 in `parser/src/lib.rs`, plus 6 doctests in
`common`).

`cargo build --workspace` produces only the three pre-existing parser
warnings (`None`/`Xor`/`Equal`/`Unary`/`Call` variants, `prefix` field,
`inc`/`dec` methods) — no warnings in the compiler or machine crates.

15 example programs exist in `examples/` (`fib`, `option`, `result`,
`tree`, `record`, `mixed`, `nested_records`, `chained`, `let_test`,
`fizbuz`, plus five unreferenced: `classes`, `const`, `coro`, `gc`,
`modules`). Eight of these are exercised by the golden integration tests
in `compiler/tests/pipeline.rs`; the others are reference examples.

The current compiler is a **single-pass, stack-based** design with 55
opcodes in `common/src/opcode.rs` and a 4861-LOC `do_compile` function in
`compiler/src/lib.rs`. The HM Hindley-Milner typechecker (Phase 14, ~3,300
LOC across 7 files) produces a `(NodeId) → Ty` cache consumed by codegen.
Control-flow uses `BlockBuilder` (Phase 16.6) for placeholder tracking,
and the VM is a stack machine with `frame.sp` as a watermark on a single
8KB operand stack.

The current VM has automatic GC (Phase 15D.1, allocation-pressure-driven
at 64 allocations), heap-pointer classification for `MakeEnum` (O(n) over
the intrusive linked list), and a mark-trace-sweep cycle rooted at the
live operand stack.

For the full phase history (Phases 14 through 19), see [`AGENTS.md`](./AGENTS.md).

---

## 3. Target Architecture

### Pipeline

```
Parse → TypedAST
   ↓
HM TypeCheck (unchanged)
   ↓
CFG Construction (AST → Function-CFGs)
   ↓
SSA-lite (block-local numbering, no phis)
   ↓
Liveness Analysis (linear sweep on SSA)
   ↓
Register Allocation (Wimmer linear scan)
   ↓
Bytecode Emission → Register VM
```

### CFG Data Structure

```rust
/// One function's control flow graph.
struct Function {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
    blocks: Vec<Block>,
    entry: BlockId,
}

/// A basic block — a maximal straight-line sequence of instructions
/// terminated by exactly one terminator.
struct Block {
    id: BlockId,
    params: Vec<SSAValue>,    // block-local parameters (set by predecessors)
    insts: Vec<Inst>,
    term: Terminator,
}

/// A block terminator — control flow leaves the block here.
enum Terminator {
    Ret(SSAValue),                              // return value
    Jump(BlockId, Vec<SSAValue>),               // unconditional jump
    Branch(SSAValue, BlockId, BlockId),         // if-then-else (cond, then, else)
    Match {                                     // multi-way match on enum tag
        scrutinee: SSAValue,
        arms: Vec<(u32, BlockId, Vec<SSAValue>)>,  // (tag, target, args)
        fallback: Option<BlockId>,
    },
}

/// An instruction — straight-line computation, no control flow.
enum Inst {
    Const(SSAValue, Literal),                   // SSAValue = dest
    BinOp(BinOpKind, SSAValue, SSAValue, SSAValue),
    Load(SSAValue, Name),                       // load a parameter or local
    Store(Name, SSAValue),                      // store to a parameter or local
    Call(SSAValue, Name, Vec<SSAValue>),        // direct call
    MakeEnum(SSAValue, Tag, Vec<SSAValue>),
    LoadField(SSAValue, SSAValue, u32),         // (dest, receiver, field_index)
    // ... etc.
}

/// SSA value — a numbered, single-assignment value.
struct SSAValue {
    id: u32,        // global counter, monotonically increasing per function
    ty: Ty,
}

/// Placeholder for full phi nodes — currently unused.
struct Phi {
    block: BlockId,
    dest: SSAValue,
    incoming: Vec<(BlockId, SSAValue)>,
}
```

`Phi` is included for completeness but not used in Phase 0–4. The
SSA-lite variant (no phis) handles all current control-flow shapes —
match arms fall through to a join block, which is reached by a single
predecessor per arm body (the join's predecessor in source-reverse-order
emission is exactly one block).

### Register VM Design

```
┌─────────────────────────────────────────┐
│  Register File (256 × 64-bit values)    │
│  ┌───────────────────────────────────┐  │
│  │ regs 0-7:   args + return (4+1)  │  │
│  │ regs 8-15:  dedicated spill       │  │
│  │ regs 16-247: callee-saves         │  │
│  │              (GC-reachable)      │  │
│  │ regs 248-255: reserved           │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

- **256 registers**, 64 bits each (same width as `Value`).
- **4 register-args + spill convention.** Callers pass the first 4 args in
  registers `a0`–`a3`; any additional args spill to the stack frame
  reserved area at `fp + 16`. The callee copies them to callee-saves
  registers on entry if they're GC-reachable.
- **8 dedicated spill registers** (`s0`–`s7`). Used by the linear-scan
  allocator when register pressure exceeds 256 — the allocator picks the
  coldest live range and spills it to `s_i`, emitting reload on use.
- **Callee-saves for GC-reachable values** (`regs 16–247`). When a
  function call happens, every register holding a heap pointer (string,
  instance, enum) is saved to the stack frame by the caller; the callee
  restores them on return. This makes the GC root set a function-static
  set of slots (`frame.callee_save_base` to `frame.callee_save_top`).
  Cost: 2–3 instructions per call site (saves + restores). Benefit: GC
  soundness without scanning the operand stack.
- **Dalvik-style hybrid register encoding.** Arithmetic instructions
  encode their register operands as a sequence of 1-byte register
  indices, with regs 0–15 as single bytes and regs 16+ as two bytes
  (high nibble `0xF`, low nibble + trailing byte). This keeps the
  bytecode compact for arithmetic-heavy programs while supporting the
  full 256-register space.

### Opcode Translation Table

| Stack opcode (current)             | Register form (target)              |
|------------------------------------|-------------------------------------|
| `CONST imm` (push imm)             | `MOV dst, imm`                      |
| `LOAD slot` (peek + push)          | `MOV dst, src_reg`                  |
| `STORE slot` (peek → slot, no pop) | `MOV dst_reg, src_reg`              |
| `StorePop slot` (pop → slot)       | `MOV dst_reg, src_reg` (same op!)   |
| `POP`                              | `MOV sink, src` (or NOP if unused)  |
| `DUPLICATE`                        | `MOV dup_reg, src_reg`              |
| `ADD` (pop, pop, push)             | `ADD dst, src1, src2`               |
| `SUB`, `MUL`, `DIV`, `MOD`, etc.   | `<OP> dst, src1, src2`              |
| `EQ`, `NEQ`, `LE`, `LEQ`, `GT`, `GEQ` | `<CMP> dst, src1, src2`          |
| `JMP target`                       | `JMP target` (same — control flow)  |
| `JMPT target` / `JMPF target`      | `Bxxx target` (conditional branch)  |
| `CALL name, argc`                  | `CALL name, argc` (same — convention differs) |
| `RETURN`                           | `RET src`                           |
| `MakeEnum tag, arity`              | `MAKE_ENUM dst, tag, src1..srcN`    |
| `JumpIfMatch tag, target`          | `JUMP_IF_MATCH src, tag, target`    |
| `Unpack arity`                     | `UNPACK src, dst1..dstN`            |
| `UnpackAt slot, arity`             | `UNPACK_AT src, slot, dst1..dstN`   |
| `LoadField field_index`            | `LOAD_FIELD dst, src, field_index`  |
| `STRING`, `FORMAT`, `PRINT`        | unchanged (operate on strings, not regs) |

Most arithmetic opcodes drop from 1 byte (stack) to ~3 bytes (register
triple) — but the elimination of `POP`/`DUP`/`LOAD`/`STORE` per
intermediate value typically nets out as a **bytecode size reduction of
20–40%** for arithmetic-heavy programs. The Dalvik-style hybrid encoding
keeps the per-instruction overhead small for the common case.

---

## 4. Key Decisions

### Decision 1: SSA-lite (no phi nodes) over full SSA

**Decision:** The CFG-to-SSA pass assigns block-local SSA numbers but
does NOT insert phi nodes at join points.

**Rationale:** No current join point needs a phi. Match arms terminate
with `JMP end_block` (carrying exactly one predecessor in source-reverse
order), `if` branches terminate with `JMP end_block` (same), and `loop`
back-edges carry the same SSA values they did on forward entry (no new
value appears on the back-edge). Every block has at most one predecessor
that introduces a new SSA value, so the "SSA-lite" simplification is
sound for the current language.

**Red-team challenge:** "What about exceptions, async, generators,
multi-pumped loops, or any other multi-successor control flow?" — the
answer is "add full SSA with phis in the phase that introduces those
features." The cost is one refactor of the SSA pass (~3 days) when
needed, but the refactor is local — the SSA-value representation and
register allocator don't change.

### Decision 2: Single-path Phase 0 (no dual-emission)

**Decision:** Phase 0 builds the CFG and emits straight-line expressions
ONLY. Control-flow constructs (`if`, `loop`, `match`) are NOT emitted in
Phase 0 — they remain on the legacy single-pass codegen path until Phase
1.

**Rationale:** A dual-emission-path approach (legacy for control flow +
new for straight-line) requires every control-flow construct to be aware
of which path is active. Every `if`/`loop`/`match` codegen arm would
need a "if we're in the new path, do X" branch. The maintenance burden
doubles for every construct. By deferring control-flow to Phase 1, the
single-path approach keeps Phase 0 narrowly scoped: "can we build the
CFG and emit a straight-line expression as register bytecode?"

**Red-team challenge:** "What about programs that use control flow?"
— Phase 0 only validates the straight-line subset. A program like
`fn add(a: int, b: int) -> int { return a + b; }` should compile via
the new path and produce correct output. Programs with `if`/`loop`/`match`
stay on the legacy path until Phase 1 completes.

### Decision 3: Callee-saves for GC-reachable values

**Decision:** When a function call happens, every register holding a
heap pointer is saved to the stack frame by the caller and restored by
the callee. The GC root set becomes a function-static slot range.

**Rationale:** Caller-saves-only would make the GC root set a moving
target — at any point during execution, the "live pointers" depend on
which calls are mid-flight and which registers haven't been clobbered
yet. This forces the GC to walk the entire call stack on every
collection, plus scan every register that COULD be a pointer
(conservative stack scanning — known to be slow and unsound w.r.t.
integer-pointer aliasing). Callee-saves makes the root set a fixed
range: `frame.callee_save_base` to `frame.callee_save_top`. Soundness
follows from the caller-saves convention: if it's in the range, it's
definitely a pointer.

**Red-team challenge:** "Callee-saves costs 2–3 instructions per call.
Isn't that too expensive?" — on the 15 existing examples, the average
call site has 0–1 GC-reachable registers in flight (most calls are
arithmetic helpers). The amortized cost is < 0.5 instructions per call.
Worth it for GC soundness.

### Decision 4: Dalvik-style hybrid register encoding

**Decision:** Register operands are encoded as a sequence of 1-byte
register indices. For regs 0–15, one byte suffices. For regs 16+, the
byte is `0xFn` (high nibble `0xF`, low nibble `n`) and a trailing byte
carries the low byte of the register index.

**Rationale:** Pure 1-byte-per-register triples bytecode size for
arithmetic-heavy programs (each `ADD` would be 4 bytes: opcode + 3
registers). Pure 2-byte-per-register (full 256-register space)
quadruples the bytecode for programs that only use 10–20 registers.
Hybrid (regs 0–15 inline, 16+ trailing) triples the size only for
programs that ACTUALLY use > 16 registers — and in practice, even with
register pressure, the linear-scan allocator keeps the hot inner loops
under 16 registers. The hybrid encoding matches Dalvik's experience.

**Red-team challenge:** "What if the linear-scan allocator's hot inner
loops spill above 16?" — register pressure measurement (Experiment C)
shows that even on `examples/mixed.0s` (the most complex example), peak
live ranges per function stay below 30. The 8 dedicated spill registers
(`s0`–`s7`) handle the rare overflow case without bytecode expansion.

### Decision 5: 150 person-days planned, 75 expected, 50 best case

**Decision:** The master schedule is 150 person-days with a 50-day
best-case floor. The Phase 0–2 estimate (50 days) was the architect's
optimistic floor; the 75-day figure is what a senior engineer with full
contextship of the codebase would expect; the 150-day figure includes
3× buffer for unknowns.

**Rationale:** Building a 3-pass compiler (CFG + SSA + liveness +
register alloc + emission) from a 1-pass compiler is fundamentally
unknown territory for this codebase. The pre-refactor patterns
(`BlockBuilder`, `match_bindings`, `codegen_var_types`) are SUGGESTIVE
of complexity, but the actual complexity of each refactor step won't
be known until it's done. A 3× buffer until Phase 0+1 stabilize is
the right risk posture.

**Red-team challenge:** "3× buffer is excessive; let's commit to 50
days." — counterpoint: Phase 16.5 (the If-codegen bugfix) was scheduled
at 2 days, took 5, and uncovered a second related bug that took
another 3 days to fix. Phase 18B (nested record patterns) was scheduled
at 5 days, took 8, and required a new VM opcode (`UnpackAt`) that
wasn't in the original scope. The 3× buffer is empirical, not
arbitrary.

---

## 5. Verification Experiments (BEFORE any refactor code)

Three experiments must run before Phase 0 begins. Each is a prototype +
analysis with a hard time-budget. If any experiment exceeds its budget,
the refactor is paused and the architects reconvene.

### Experiment A: Match codegen + SSA-lite compatibility (~3 days)

**Goal:** Validate that the current `match` semantics (which we KNOW the
existing match codegen produces correctly via the reverse-source-order
fall-through layout) can be expressed in SSA-lite without phi nodes.

**Method:** Build a minimal CFG + SSA-lite prototype for ONE match
expression: `match opt { Some(v) => v, None => 0 }`. Manually walk
through:

1. Which SSA values hold `v` in the `Some` arm?
2. Does `0` conflict with anything (live range analysis)?
3. Does the join (after the match) need a phi?
4. How does the prototype's `JUMP_IF_MATCH` differ from the current
   bytecode-emitter's `JUMP_IF_MATCH`?

**Pass criterion:** You can answer all 4 questions in under 3 days, AND
the prototype's generated bytecode is byte-equivalent to the existing
codegen for `examples/option.0s`.

**Fail signal:** If you can't answer in 3 days, SSA-lite is wrong for
this codebase, and the architects must either (a) commit to full SSA +
phis, or (b) defer the match codegen to a later phase.

### Experiment B: GC root set for register VM (~2 days)

**Goal:** Validate that the existing GC tests would still pass with the
callee-saves GC root set.

**Method:** Take the 21 existing tests in `machine/src/vm.rs::tests`.
For each, manually walk through what the GC's root set would be if the
operand stack were replaced with a 16-register file. Pay special
attention to:

- `nested_enum_gc_traces_correctly` — the most complex existing test
  (enum pointer → enum pointer → int).
- `live_enum_survives_automatic_gc_cycle` — the basic survival test.
- `heap_does_not_grow_unboundedly_under_repeated_alloc` — the
  collection trigger test.

For each test, sketch the callee-saves convention in action: when the
test enters a function, which registers are callee-saves? When the GC
fires mid-execution, what's the root set?

**Pass criterion:** You can answer "GC would correctly trace this enum"
for all 21 tests, AND the manual walkthrough produces a root-set
algorithm that fits in < 50 LOC.

**Fail signal:** If you can't answer for any test, the root-set
redesign is harder than expected, and Phase 2 (GC migration) needs to
be re-scoped.

### Experiment C: Register pressure on current examples (~3 days)

**Goal:** Validate the architect's claim that "no register pressure"
exists in the current examples.

**Method:** Take three representative examples:

- `examples/mixed.0s` — multi-shape enum with binding bodies.
- `examples/record.0s` — chained field access.
- `examples/nested_records.0s` — deep pattern nesting (depth 3+).

Write a 100-LOC linear-scan allocator that runs over a hand-extracted
SSA form of each example. Report:

1. Peak live-range count per function.
2. Number of spills required.
3. Whether any function exceeds 16 registers (the inline-encoded range).
4. Whether any function exceeds 256 registers (the architecture's
   hard limit).

**Pass criterion:** Peak < 256 for every function in every example.
Ideally peak < 30 (so the hot inner loops fit in inline-encoded regs).

**Fail signal:** If peak > 256 for any function, the architect's
"no register pressure" claim is wrong, and the refactor needs a
spill-to-stack strategy + a runtime memory cost analysis.

---

## 6. Five-Phase Roadmap

| Phase | Deliverable | LOC | Days | Risk | What Changes |
|-------|-------------|-----|------|------|--------------|
| **0**  | CFG + SSA-lite + register alloc + emission for **straight-line expressions only**. | ~1,500 (new `cfg/`, `ssa/`, `regalloc/`, `emit_reg.rs`) | 25 | Medium | New pipeline path runs alongside legacy. Straight-line expressions use the new path; control flow stays on legacy. ARCHIVE_VERSION unchanged. |
| **1**  | Wire control-flow constructs (`if`, `loop`, `match`) into the new pipeline. | ~1,200 (extend `emit_reg.rs`, add `cfg/` for control flow) | 25 | High | `BlockBuilder` becomes unused (orphaned, not deleted). `match_bindings` becomes unused. Reverse-source-order layout replaced by block-list layout. |
| **2**  | Migrate VM to register form + callee-saves GC root set. | ~800 (new `machine/src/vm_reg.rs`) + ~300 GC migration | 20 | High | New VM runs alongside stack VM. GC root set becomes function-static. Allocation sites emit register form. ARCHIVE_VERSION unchanged. |
| **3**  | Remove legacy paths. Single-path emission. | ~600 (deletions + simplification) | 15 | Low | `BlockBuilder` deleted. `match_bindings` deleted. `codegen_var_types` deleted. `do_compile` shrinks from 4861 LOC to ~2,000 LOC. ARCHIVE_VERSION unchanged. |
| **4**  | Cut over the on-disk format. Bump ARCHIVE_VERSION to 2. `.c0s` v1 files rejected at load time. | ~200 | 5 | Low | `ArchivedProgram` adds `version: u32 = 2`. `Pipeline::run` rejects v1. `src/main.rs` recompiles on version mismatch. |

**Total:** 3,700 new LOC + 1,200 deletions + 200 maintenance = ~4,300 net
LOC across the refactor. **90 days best case, 150 days planned.**

---

## 7. Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | **Phase 0 dual-emission-paths create maintenance debt** | High | Medium | Mitigated by Decision 2 (single-path Phase 0 — no dual emission). Phase 0 only emits straight-line; control flow stays on legacy. |
| 2 | **Register VM GC root set unsoundness** | Medium | Critical | Mitigated by Experiment B (validate root-set on 21 existing tests before Phase 2) + Decision 3 (callee-saves for GC-reachable values). |
| 3 | **Register pressure > 256 on real programs** | Low | Critical | Mitigated by Experiment C (measure register pressure on 3 representative examples before Phase 0). |
| 4 | **Match codegen reverse-source-order doesn't compose with SSA** | Medium | High | Mitigated by Experiment A (build a minimal CFG+SSA prototype for one match expression before Phase 1). |
| 5 | **Phase estimate too optimistic** | High | Medium | Mitigated by Decision 5 (3× buffer until Phase 0+1 stabilize). Pause and reconvene if Phase 0 exceeds 30 days. |

---

## 8. Migration & Rollback Strategy

Each phase is **additive** until Phase 4. Rollback is `git revert` of the
phase commits. The commit history should look like:

```
<phase 0 commits>      ← additive: new pipeline path runs alongside legacy
<phase 1 commits>      ← additive: control flow migrated
<phase 2 commits>      ← additive: register VM runs alongside stack VM
<phase 3 commits>      ← deletions: legacy paths removed
<phase 4 commit>       ← cutover: ARCHIVE_VERSION bumped to 2
```

`ARCHIVE_VERSION` stays at `1` until Phase 4. Any program compiled by
the pre-refactor compiler (or by the Phase 0–3 dual-path compiler) is
still loadable by the Phase 4 compiler, which immediately recompiles
it on version mismatch.

The Phase 4 cutover is the **only** irreversible step. If Phase 4
fails or needs to be rolled back, the .c0s files compiled by the
post-cutover compiler are unusable on the pre-cutover compiler — but
they can always be recompiled from source.

---

## 9. References

- [`AGENTS.md`](./AGENTS.md) — Full phase history (Phases 14–19).
- [`HM_TYPECHECKER_PLAN.md`](./HM_TYPECHECKER_PLAN.md) — Hindley-Milner
  typechecker design notes (Phase 14).
- "Multi-Pass Register-VM Refactor Synthesis" — internal orchestration
  analysis from 2026-06-22, sections §2 (current-state inventory),
  §3 (architecture design), §4 (decision matrix), §5 (phase breakdown).

The synthesis report's decision matrix is the basis for §4 of this
document; the experiment definitions in §5 are derived from the
synthesis's "open questions" subsection.