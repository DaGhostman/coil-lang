# Refactor Verification Experiments

This directory contains prototype code and analysis for the three verification
experiments defined in `../MULTI_PASS_REFACTOR_PLAN.md` §5. These experiments
MUST complete before any Phase 0 refactor code is written.

## Rules

1. **Prototypes only.** Code in this directory is exploratory and is NOT
   part of the production compiler. It may not compile with the main
   workspace, may have placeholder implementations, and is explicitly
   excluded from `cargo test --workspace`.

2. **One experiment per subdirectory.** No cross-directory imports.
   Each experiment is self-contained.

3. **Each experiment has a README.md** at its root documenting:
   - What question it answers
   - What success looks like
   - How to run it
   - What was learned (filled in as the experiment progresses)

4. **Results feed back into MULTI_PASS_REFACTOR_PLAN.md.** When an
   experiment concludes, the relevant section of the plan document is
   updated (in a separate commit) with the experiment's findings.

## Experiment A: Match Codegen + SSA-lite Compatibility

**Subdirectory:** `match_ssa_lite/`

**Question:** Does the current match codegen's reverse-source-order arm
emission compose with SSA-lite (block-local numbering)?

**What we'll build:**
- A minimal CFG + SSA-lite prototype for one match expression
- Walk through `match opt { Some(v) => v, None => 0 }` by hand
- Identify which SSA values hold `v` in the Some arm
- Check whether the join at match-end needs a phi

**Success criterion:** Within 3 days, we can answer "which SSA values
hold v?" and "does the join need a phi?" with confidence.

**Risk if it fails:** SSA-lite is wrong for this codebase. We revisit
the SSA decision (use full SSA, or use no SSA at all and rely on the
existing `match_bindings` workaround).

**Estimated effort:** 3 person-days.

## Experiment B: GC Root Set for Register VM

**Subdirectory:** `gc_root_set/`

**Question:** How does the current GC's root set (the operand stack as a
flat array) translate to a register-VM root set?

**What we'll build:**
- A walkthrough of the 11 existing GC tests in `machine/src/vm.rs::tests`
- For each, manually determine what the GC's root set would be if the
  operand stack were replaced with a 16-register file
- Pay special attention to `nested_enum_gc_traces_correctly`

**Success criterion:** Within 2 days, we can answer "GC would correctly
trace this enum" for every existing GC test.

**Risk if it fails:** The root-set redesign is harder than expected.
We may need a different calling convention (e.g., more callee-saves
registers) or a different GC design (e.g., card marking).

**Estimated effort:** 2 person-days.

## Experiment C: Register Pressure on Current Examples

**Subdirectory:** `regalloc_pressure/`

**Question:** Is 256 registers enough for the existing examples, and
where does register pressure actually peak?

**What we'll build:**
- A 100-LOC linear-scan register allocator prototype
- Run it on `examples/mixed.0s`, `examples/record.0s`,
  `examples/nested_records.0s`
- Report peak live-range count per function and number of spills

**Success criterion:** Within 3 days, we have measured numbers.

**Risk if it fails:** Register pressure exceeds 256 in baseline
examples. We raise the ceiling (256 → 512 or 1024), accept spills in
baseline examples, or redesign the encoding (e.g., stack/register
hybrid).

**Estimated effort:** 3 person-days.

## Order of execution

Experiments A, B, and C are independent and CAN run in parallel.
However, since we have a single developer, the recommended order is:

1. **A first** (3 days) — biggest architectural risk
2. **B second** (2 days) — second-biggest risk, but small
3. **C third** (3 days) — least architectural risk, mostly measurement

Total: 8 person-days. After all three complete, update
`MULTI_PASS_REFACTOR_PLAN.md` with findings (separate commit) and
proceed to Phase 0.
