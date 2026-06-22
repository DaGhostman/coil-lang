# Experiment A: Match Codegen + SSA-lite Compatibility

## Question

Does the current match codegen's reverse-source-order arm emission
compose with **SSA-lite** (block-local SSA value numbering, no
dominance frontiers, no phi-nodes)?

## What we built

A standalone Rust prototype at `src/main.rs` that:

1. Defines minimal CFG types (`Function`, `Block`, `Terminator`,
   `Inst`, `ValueId`, `BlockId`)
2. Builds a CFG for `fn unwrap_or_zero(opt: Option) -> int { return
   match opt { Some(v) => v, None => 0 }; }`
3. Numbers SSA values with block-local IDs (no phi-nodes)
4. Analyzes live ranges and phi-requirement

## How to run

```bash
cd experiments/match_ssa_lite
cargo run
```

Expected output: a CFG dump, SSA value analysis, and a decision
section confirming that SSA-lite is sufficient.

## Findings

**SSA-lite is sufficient.** The canonical match expression has two
arms, each ending with its own RETURN instruction. No block receives
live values from both arms. Therefore no phi-nodes are needed.

The current match codegen's reverse-source-order emission (Phase 15C)
composes cleanly with block-local SSA numbering:
- Each arm body is its own block
- Each arm's bindings are fresh SSA values (no global `match_bindings`
  map collision)
- The dispatch block (Block 0) emits a `JumpIfMatch` for non-last
  arms; the last arm falls through

## Implications for the refactor

The architectural decision in `MULTI_PASS_REFACTOR_PLAN.md` §4
("SSA-lite over full SSA") is **validated**.

We do NOT need to:
- Compute dominance frontiers
- Place phi-nodes at join points
- Implement phi-elimination during register allocation

We CAN:
- Use simple block-local value counters (`next_value += 1` per block)
- Map SSA values directly to registers during linear-scan allocation
- Avoid the complexity tax of full SSA

## Follow-up

When zero-script adds features that REQUIRE phi-nodes:
- Exceptions (catch handlers create join points across many blocks)
- Async/await (futures create implicit join points at `await`)
- Closures (captured variables create implicit join points)

At that point, promote to full SSA. The cost is implementing
dominance frontiers (~200 LOC) and phi-placement (~100 LOC). Until
then, SSA-lite is the right tool.
