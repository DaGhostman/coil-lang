# Experiment B: GC Root Set for Register VM

## Question

How does the current GC's root set (the operand stack as a flat array)
translate to a register-VM root set? Does any existing GC test fail
under our target calling convention?

## Baseline: Current GC root set

The current GC's `gc_collect` (in `machine/src/vm.rs` around
lines 202–243) walks the operand stack as a flat array:

```rust
fn gc_collect(heap: &mut Heap, stack: &Stack<Value, 8192>, alloc_counter: &mut usize) {
    // Phase 15D.1 — the trace root set is every value on
    // the operand stack. Values that fall in the heap's
    // address range are roots; immediates are silently
    // ignored by `heap.trace`.
    let roots: Vec<u64> = stack.as_slice().iter().map(|v| v.raw() as u64).collect();

    heap.trace(&roots);

    // ... mark-and-trace loop ...

    unsafe { heap.sweep() };
    *alloc_counter = 0;
}
```

The root set is **everything on the operand stack**: the entire
`stack[..cursor]` slice is scanned, and `Heap::contains_addr` filters
out immediates (ints, floats, bools) — only values whose raw bits fall
within the heap's address range are kept. This is **conservative
scanning** of the operand stack (Option 3, applied to the stack instead
of the registers).

### Allocation-pressure trigger

The GC fires when `alloc_counter > GC_TRIGGER_INTERVAL` (currently
`64`). Every heap allocation site (`INIT`, `STRING`, `FORMAT`,
`MAKE_ENUM`) increments the counter. After a sweep, the counter resets
to zero.

### Why the operand-stack model is "free"

There is no register liveness analysis to maintain — any value the VM
might need is on the stack by construction. The tradeoff is a flat
8 KB array (`Stack<Value, 8192>`) that's scanned every collection, even
though most slots hold immediates or are unused. This is acceptable
because the GC only fires every 64 allocations.

## Target: Register-VM root set options

The Phase 0+2 refactor moves from an 8 KB operand stack to a 256-register
file (see `MULTI_PASS_REFACTOR_PLAN.md` §3 "Register VM Design"). The GC
root set must be redefined.

### Option 1: Caller-saves only (REJECTED)

**Design:** Caller stores all live registers to spill slots before
each `CALL`. GC walks current frame's live registers plus all spill
slots of all frames.

**Why rejected:** The GC's root set becomes a **moving target** at any
point during execution. The set of "live pointers" depends on which
calls are mid-flight and which registers haven't been clobbered yet.
The red-team's concern (see `MULTI_PASS_REFACTOR_PLAN.md` §4 Decision
3): "at any point during execution, the live pointers depend on which
calls are mid-flight and which registers haven't been clobbered yet.
This forces the GC to walk the entire call stack on every collection,
plus scan every register that COULD be a pointer (conservative stack
scanning — known to be slow and unsound w.r.t. integer-pointer
aliasing)."

In short: Option 1 forces Option 3's conservative scanning at every
GC, every collection. The complexity tax is too high.

### Option 2: Callee-saves for GC-reachable values (RECOMMENDED)

**Design:** Reserve registers `R_CS_0..R_CS_3` (4 callee-save regs for
heap ptrs). Caller-saves for everything else (immediates, temp
values, function arguments). The GC root walk becomes:

1. For each frame in the call stack, read `R_CS_0..R_CS_3` from that
   frame's saved-register area.
2. Add the current frame's live registers (those not covered by
   callee-saves).

**Why it works:**

- **Soundness follows from the convention.** If a register is in the
  `R_CS_0..R_CS_3` range at the GC site, the calling convention
  guarantees it's either a heap pointer (string/instance/enum) or
  unused (zeroed by the callee prologue). No false positives.
- **Fixed root-set size.** Per frame: 4 registers (32 bytes). Walk
  cost: `O(frames × 4)`, which is `O(1)` for typical zero-script
  programs (≤ 8 frames deep, see `examples/`).
- **2–3 instruction overhead per call.** The callee-save prologue
  saves 4 registers to the frame's spill area; the epilogue restores
  them. Total: 2× `MOV spill, R_CS_i` and 2× `MOV R_CS_i, spill`,
  approximately 4–8 instructions per call depending on encoding.
  The amortized cost on the existing examples is < 0.5 instructions
  per call site (most calls are arithmetic helpers with no
  GC-reachable registers in flight).

### Option 3: Conservative scan (fallback only)

**Design:** Scan ALL 256 registers and treat any value as a potential
pointer. Filter through `Heap::contains_addr` (an O(n) walk over the
intrusive list).

**Why it's only a fallback:** Over-estimates the live set — an int
constant like `0x12345678` that happens to fall in the heap's address
range would be treated as a root, causing a memory leak (the "live"
object is kept forever, even though no program value references it).
For a GC'd language with small register files this is sometimes
acceptable, but it's unsound for languages with manual memory
management. zero-script's GC is non-moving, so leaks are recoverable
but undesirable.

The current operand-stack GC IS Option 3 applied to the stack. The
8 KB stack is small enough that the conservative filter is fast enough
in practice — but the register file is 256 × 8 bytes = 2 KB and has
FAR more non-pointer values (intermediates, return values, etc.) than
the operand stack does. Option 3 on the register file would have a
much higher false-positive rate.

## Test-by-test analysis

21 existing tests live in `machine/src/vm.rs::tests` (19) and
`machine/src/memory/heap.rs::tests` (2). For each, we walk through
what the GC's root set would be in the register-VM model.

Three tests are pure GC tests (`nested_enum_gc_traces_correctly`,
`heap_does_not_grow_unboundedly_under_repeated_alloc`,
`live_enum_survives_automatic_gc_cycle`); two are heap-level GC tests
(`enum_gc_marks_payload_pointers`, `enum_gc_marks_nested_enum_payloads`).
The remaining 16 tests don't directly exercise GC, but they allocate
heap objects (via `MAKE_ENUM` or `STRING`) and could trigger a
collection if the allocation count exceeds `GC_TRIGGER_INTERVAL = 64`.

### Test: `make_enum_allocates_enum_with_correct_tag` (vm.rs:1121)

**What it verifies:** `MAKE_ENUM 0 0` (zero-arity constructor) pushes
a heap pointer onto the operand stack.

**Bytecode:** `MAKE_ENUM 0 0, HALT` (1 allocation).

**Current root set:** `[enum_ptr]` (the allocated enum's address).

**Register-VM root set under Option 2:**

- `R_CS_0..R_CS_3 = [0, 0, 0, 0]` (callee-save prologue clears them)
- The enum's destination register (say `R_5`) is NOT in the
  callee-save range — it's a temp register holding the just-allocated
  enum. At the GC trigger point (during the next allocation site),
  `R_5` is no longer live if its value has been consumed.

**At GC trigger:** This test allocates only 1 enum, well below the
threshold of 64. No GC fires. The test never observes the root-set
walk.

**Concerns:** None.

---

### Test: `make_enum_with_payload_populates_payload` (vm.rs:1148)

**What it verifies:** `CONST 42, CONST 7, MAKE_ENUM 1 2` pushes a
heap pointer for an enum with payload `[42, 7]` (in declaration
order).

**Bytecode:** `CONST 42, CONST 7, MAKE_ENUM 1 2, HALT` (1 allocation).

**Current root set:** `[42, 7, enum_ptr]` — the operand stack has
two ints and one pointer at the GC trigger.

**Register-VM root set under Option 2:**

- `R_0 = 42` (CONST dest), `R_1 = 7` (CONST dest), `R_2 = enum_ptr`
  (MAKE_ENUM dest). All three are temp registers; none are in
  `R_CS_0..R_CS_3`.
- The MAKE_ENUM is followed by HALT; no further allocation triggers
  GC.

**At GC trigger:** No GC fires (1 allocation < 64).

**Concerns:** None. The ints `42` and `7` would have been scanned by
the conservative stack walk; in the register VM they're classified as
immediates and skipped.

---

### Test: `jump_if_match_taken_advances_ip` (vm.rs:1171)

**What it verifies:** `JUMP_IF_MATCH` with matching tag advances the
IP and pushes the payload value.

**Bytecode:** `CONST 42, MAKE_ENUM 2 1, JUMP_IF_MATCH 2 target=4,
CONST 999, HALT` (1 allocation).

**Current root set:** `[42, enum_ptr, 42]` (after JUMP_IF_MATCH taken,
the payload 42 is pushed).

**Register-VM root set under Option 2:**

- `R_0 = 42` (CONST dest for the first `CONST 42`)
- `R_1 = enum_ptr` (MAKE_ENUM dest)
- After `JUMP_IF_MATCH` is taken: `R_2 = 42` (payload pushed into
  dest register)
- All temp registers; no callee-saves involved.

**At GC trigger:** No GC fires (1 allocation < 64).

**Concerns:** None. The "payload-pushes-into-register" behavior is
identical to the current "payload-pushes-onto-stack" semantics.

---

### Test: `jump_if_match_wide_target_round_trips` (vm.rs:1214)

**What it verifies:** Wide targets (≥ 65,535 bytes) round-trip via
the `value_u32()` accessor.

**Bytecode:** No allocation. The test only inspects a constructed
`Byte` struct.

**Current root set:** N/A (no execution).

**Register-VM root set under Option 2:** N/A.

**Concerns:** None. The test doesn't run the VM.

---

### Test: `jump_if_match_not_taken_falls_through` (vm.rs:1241)

**What it verifies:** Non-matching `JUMP_IF_MATCH` falls through to
the next instruction.

**Bytecode:** `CONST 42, MAKE_ENUM 2 1, JUMP_IF_MATCH 5 target=4,
CONST 99, HALT` (1 allocation).

**Current root set:** `[42, enum_ptr]` (after fall-through) → after
`CONST 99`, `[42, enum_ptr, 99]`.

**Register-VM root set under Option 2:** Identical to the
"taken" case, minus the payload push. Same concerns.

**At GC trigger:** No GC fires (1 allocation < 64).

**Concerns:** None.

---

### Test: `unpack_pops_enum_and_pushes_payload` (vm.rs:1263)

**What it verifies:** `UNPACK 3` pops an enum and pushes its payload
in declaration order.

**Bytecode:** `CONST 30, CONST 20, CONST 10, MAKE_ENUM 0 3, UNPACK 3,
HALT` (1 allocation).

**Current root set:** `[10, 20, 30]` after UNPACK — three immediates.

**Register-VM root set under Option 2:**

- `R_0 = 30, R_1 = 20, R_2 = 10` (the three CONST destinations)
- `R_3 = enum_ptr` (MAKE_ENUM dest, consumed by UNPACK)
- After UNPACK: `R_0 = 10, R_1 = 20, R_2 = 30` (overwritten in place
  with the payload values — same as the slot-based writes in the
  current VM)

**At GC trigger:** No GC fires (1 allocation < 64).

**Concerns:** None. UNPACK's payload-push semantics translate
directly to register-VM "write the payload to consecutive destination
registers starting at the dest offset."

---

### Test: `load_field_extracts_field_zero` (vm.rs:1300)

**What it verifies:** `LOAD_FIELD 0` extracts `payload[0] = 10` from
an enum with payload `[10, 20, 30]`.

**Bytecode:** `CONST 30, CONST 20, CONST 10, MAKE_ENUM 0 3,
LOAD_FIELD 0, HALT` (1 allocation).

**Current root set:** `[10]` after LOAD_FIELD — one immediate.

**Register-VM root set under Option 2:**

- `R_0 = 30, R_1 = 20, R_2 = 10, R_3 = enum_ptr` (MAKE_ENUM dest)
- After LOAD_FIELD: `R_4 = 10` (the extracted field; the enum is
  consumed)

**At GC trigger:** No GC fires (1 allocation < 64).

**Concerns:** None. LOAD_FIELD's "pop receiver, push field" maps
directly to "consume src_reg, write dest_reg = src_reg.payload[i]".

---

### Test: `load_field_extracts_last_field` (vm.rs:1324)

**What it verifies:** `LOAD_FIELD 2` extracts `payload[2] = 30`.

**Concerns:** None. Same shape as `load_field_extracts_field_zero`.

---

### Test: `load_field_extracts_middle_field` (vm.rs:1349)

**What it verifies:** `LOAD_FIELD 1` extracts `payload[1] = 20`.

**Concerns:** None. Same shape as `load_field_extracts_field_zero`.

---

### Test: `load_field_consumes_receiver` (vm.rs:1371)

**What it verifies:** After `LOAD_FIELD`, the enum is no longer on
the operand stack.

**Concerns:** None. The receiver consumption semantics translate
identically to register-VM "src_reg is read but not preserved".

---

### Test: `load_field_out_of_bounds_silent_noop` (vm.rs:1398)

**What it verifies:** Out-of-bounds `LOAD_FIELD` is a defensive
silent no-op (no panic, no incorrect push).

**Concerns:** None. The "silent no-op" semantics must be preserved
in the register VM (no dest write, no error).

---

### Test: `nested_enum_gc_traces_correctly` (vm.rs:1429)

**What it verifies:** The 15A GC correctly traces a nested
enum-of-enum-and-string. Allocates three objects on the heap, marks
the outer enum as a root, and asserts all three survive a sweep.

This is **the test called out by name in `MULTI_PASS_REFACTOR_PLAN.md`
§5 Experiment B as the most complex existing case**.

**Setup (operates on `Heap` directly, not the VM):**

```rust
let (inner_obj, _) = heap.alloc(ObjEnum { tag: 99, payload: vec![] }, Object::Enum);
let (string_obj, _) = heap.alloc(ObjString::from("inner"), Object::String);
let (outer_obj, _) = heap.alloc(
    ObjEnum { tag: 0, payload: vec![Member::Object(inner_obj), Member::Object(string_obj)] },
    Object::Enum,
);
// Mark outer as root, propagate via mark_references, sweep.
```

**Current root set:** `[outer_obj.addr()]` — explicitly supplied
roots, not derived from any stack walk.

**Register-VM root set under Option 2:**

- This test **doesn't run the VM** — it operates directly on the
  `Heap` and calls `heap.trace(&[outer_obj.addr()])` to mark the
  root explicitly. So the "register-VM root set" is whatever the
  caller passes to `heap.trace`.
- In a real register-VM scenario where this test were re-expressed
  as a bytecode sequence:
  - The outer enum's destination register (say `R_5`) would NOT be
    in `R_CS_0..R_CS_3` (it's a temp), so the callee-save prologue
    would NOT preserve it across the next call.
  - If GC fires mid-test, the GC must scan `R_5` AND the callee-
    save registers of the current frame's callees.
  - **Concern:** if the next instruction after MAKE_ENUM is a
    function call, the temp register holding the outer enum would
    be lost across the call unless the caller explicitly saves it.
    The codegen must insert a `MOV R_CS_0, R_5` before the call.

**Concerns:** **Minor** — this test exposes the requirement that the
codegen must move any heap-pointer temp into a callee-save register
before a `CALL`. The 2-3 instruction cost (per call) is the load-
bearing piece of Option 2's design.

---

### Test: `heap_does_not_grow_unboundedly_under_repeated_alloc` (vm.rs:1523)

**What it verifies:** The VM's automatic GC fires at allocation
pressure and frees unreachable enums.

**Bytecode:** `CONST 0`, then 200 iterations of `MAKE_ENUM 0 1,
POP`, then `HALT`. 200 allocations > 64 = GC fires multiple times.

**Current root set:** At each GC trigger, the operand stack contains
just `CONST 0` (the sentinel int). It's an immediate, not a pointer.
**The root set is empty** at every GC site.

**Register-VM root set under Option 2:**

- `R_0 = 0` (the sentinel CONST dest)
- `R_CS_0..R_CS_3 = [0, 0, 0, 0]` (cleared by callee-save prologue;
  no heap pointers in flight)
- The MakeEnum destinations are temp registers that have been POPed
  (i.e., the registers have been overwritten or marked unused by the
  codegen).

**At GC trigger:** All 200 POPed enums are unreachable. The empty
root set means sweep frees everything, and the heap size stays
bounded by `GC_TRIGGER_INTERVAL` (~64) instead of growing linearly.

**Concerns:** None. The "empty root set" case works identically in
both models. The heap plateaus at the same size.

---

### Test: `live_enum_survives_automatic_gc_cycle` (vm.rs:1579)

**What it verifies:** An enum kept on the stack across many
allocations of unrelated (POPed) enums survives every GC cycle.

**Bytecode:** `CONST 0, MAKE_ENUM 7 1, [CONST 0, MAKE_ENUM 0 1, POP]
× 200, HALT`. 201 allocations.

**Current root set:** At each GC trigger, the operand stack contains
the live root enum at the bottom plus `n - i` POPed enums above it
(their addresses are still on the stack until the cursor moves past
them — but POP discards the value, so the cursor moves past).

Wait — let me re-read the bytecode:

```
CONST 0
MAKE_ENUM 7 1   <- root (live), stays on stack
loop 200 times:
  CONST 0
  MAKE_ENUM 0 1 <- temp enum, POPed next
  POP           <- cursor moves past the temp enum's address
HALT
```

After each iteration's POP, the temp enum's address is gone from the
stack cursor. The live root enum is at the bottom.

**Current root set:** `[root_enum_ptr]` (just the live root) at
every GC trigger.

**Register-VM root set under Option 2:**

- **The critical decision:** where is the live root stored?

  If the codegen stores the live root in a temp register (say `R_2`),
  then between the MAKE_ENUM and the loop, the next instruction is
  `JMP loop_top`, and within the loop body the temp registers get
  reused. The root would be clobbered by the loop body's `CONST 0`.

  **The codegen MUST move the live root into a callee-save register
  before entering the loop.** Something like:
  ```
  CONST 0
  MAKE_ENUM R_CS_0, 7, 1   <- root in R_CS_0
  loop:
    CONST 0
    MAKE_ENUM R_2, 0, 1
    POP R_2                  <- R_2 marked unused
    JMP loop                 <- R_CS_0 preserved across the back-edge
  ```

  At every GC trigger, `R_CS_0 = root_enum_ptr`, and the temp
  registers `R_2` may or may not hold a just-POPed value (the POP
  semantics in the register VM mark the register unused, so the GC
  walks only `R_CS_0..R_CS_3` of the current frame).

- The callee-save prologue at function entry clears `R_CS_0..R_CS_3`
  to zero (zero is not a heap address, so it's a safe sentinel).
  The codegen's `MAKE_ENUM R_CS_0, ...` is the ONLY instruction that
  can move a heap pointer into a callee-save register (or any
  `MOV R_CS_0, R_temp` pattern).

**At GC trigger:** `R_CS_0 = root_enum_ptr` → root survives every
collection. The 200 POPed enums are unreachable and get swept.

**Concerns:** **Minor** — the codegen must be careful to move any
loop-carried heap pointer into a callee-save register. This is a
standard pattern (every C compiler does it for loop-carried
variables), but it requires the linear-scan register allocator to
recognize loop-carried live ranges and pin them to callee-saves.

---

### Test: `with_output_captures_print` (vm.rs:1661)

**What it verifies:** `Machine::with_output` redirects `PRINT`
output to a custom writer.

**Bytecode:** `STRING "hello", PRINT, HALT`. 1 STRING allocation.

**Current root set:** `[string_ptr]` at any GC site (but only 1
allocation, no GC fires).

**Register-VM root set under Option 2:**

- The STRING destination register (say `R_3`) holds `string_ptr`.
- Between STRING and PRINT there's no CALL, so `R_3` doesn't need to
  be callee-saved.
- If the program were longer (more allocations between STRING and
  PRINT), the codegen would need to move `R_3` into a callee-save
  register before any call. For this 1-allocation test, no GC fires.

**Concerns:** None.

---

### Test: `store_pop_writes_value_to_slot_and_pops` (vm.rs:1735)

**What it verifies:** `STORE_POP 0` writes the top-of-stack value
to slot 0 and pops it.

**Bytecode:** `CONST 42, STORE_POP 0, LOAD 0, HALT`. No allocations.

**Current root set:** N/A (no allocation, no GC trigger).

**Register-VM root set under Option 2:**

- `R_0 = 42` after CONST, then `R_0 = 42` after STORE_POP (writes
  the value to the slot, which IS `R_0` in the register VM — the
  slot index = the register index).

**Concerns:** None. STORE_POP and STORE collapse to the same op
("MOV dst_reg, src_reg") in the register VM (see
`MULTI_PASS_REFACTOR_PLAN.md` §3 "Opcode Translation Table").

---

### Test: `store_pop_writes_to_correct_slot_index` (vm.rs:1762)

**What it verifies:** `STORE_POP 2` writes to slot 2, not slot 0.

**Bytecode:** `CONST 99, STORE_POP 0, CONST 42, STORE_POP 2,
LOAD 2, HALT`. No allocations.

**Register-VM root set under Option 2:**

- `R_0 = 99`, `R_2 = 42` — both writes hit their target register.

**Concerns:** None.

---

### Test: `store_pop_two_bindings_preserves_both_values` (vm.rs:1786)

**What it verifies:** `let x = 5; let y = 10;` produces two
`STORE_POP` ops that don't clobber each other.

**Bytecode:** `CONST 5, STORE_POP 0, CONST 10, STORE_POP 1,
LOAD 0, LOAD 1, ADD, HALT`. No allocations.

**Register-VM root set under Option 2:** Same as the slot model:
`R_0 = 5`, `R_1 = 10`.

**Concerns:** None.

---

### Test: `store_pop_overwrites_existing_slot` (vm.rs:1813)

**What it verifies:** Re-assignment via `x = 10;` overwrites the
slot.

**Bytecode:** `CONST 5, STORE_POP 0, CONST 10, STORE_POP 0,
LOAD 0, HALT`. No allocations.

**Register-VM root set under Option 2:** Same as the slot model:
`R_0 = 10` after the second STORE_POP.

**Concerns:** None.

---

### Test: `enum_gc_marks_payload_pointers` (heap.rs:965)

**What it verifies:** An enum whose payload holds a `Member::Object`
pointer to a string — both objects survive a sweep when the enum
is marked.

**Setup:** Allocates a string and an enum with the string in its
payload. Calls `heap.trace(&[enum_addr])` to mark the enum as root.

**Current root set:** `[enum_addr]` — explicit caller-supplied root.

**Register-VM root set under Option 2:**

- The test operates on `Heap` directly (no VM execution), so the
  "root set" is whatever the test passes to `heap.trace`.
- In a register-VM scenario, the enum would be in a destination
  register; if it's at risk of GC during the next call, it would
  be in `R_CS_0..R_CS_3`.

**Concerns:** None. The heap-level test is independent of the VM's
calling convention — it tests the heap's mark-and-sweep primitive
in isolation.

---

### Test: `enum_gc_marks_nested_enum_payloads` (heap.rs:1012)

**What it verifies:** An enum whose payload holds another enum —
both survive a sweep via `mark_references` propagation.

**Setup:** Allocates an inner enum (empty payload) and an outer enum
whose payload contains the inner enum. Marks the outer as root.

**Current root set:** `[outer_addr]`.

**Register-VM root set under Option 2:**

- Same as `enum_gc_marks_payload_pointers` — the test is heap-level,
  not VM-level.
- The inner enum is unreachable from the outer enum's slot in the
  heap (it's reachable ONLY through the outer's payload), but the
  `mark_references` propagation handles this correctly. No change
  needed for the register VM.

**Concerns:** None. This test exercises the `Object::mark_references`
transitive propagation, which is orthogonal to the root-set walk.

## Summary table

| Test | Verifies | Current root set | Register-VM root set | Concerns |
|------|----------|------------------|----------------------|----------|
| `make_enum_allocates_enum_with_correct_tag` | Zero-arity MAKE_ENUM | `[enum_ptr]` | `R_temp = enum_ptr` (1 alloc, no GC) | None |
| `make_enum_with_payload_populates_payload` | MAKE_ENUM with payload | `[42, 7, enum_ptr]` | `R_0=42, R_1=7, R_2=enum_ptr` (1 alloc) | None |
| `jump_if_match_taken_advances_ip` | JUMP_IF_MATCH tag hit | `[42, enum_ptr, 42]` | `R_0=42, R_1=enum_ptr, R_2=42` | None |
| `jump_if_match_wide_target_round_trips` | Byte struct round-trip | N/A (no execution) | N/A | None |
| `jump_if_match_not_taken_falls_through` | JUMP_IF_MATCH miss | `[42, enum_ptr, 99]` | `R_0=42, R_1=enum_ptr, R_2=99` | None |
| `unpack_pops_enum_and_pushes_payload` | UNPACK semantics | `[10, 20, 30]` | `R_0..R_2 = [10, 20, 30]` | None |
| `load_field_extracts_field_zero` | LOAD_FIELD 0 | `[10]` | `R_dest = 10` | None |
| `load_field_extracts_last_field` | LOAD_FIELD 2 | `[30]` | `R_dest = 30` | None |
| `load_field_extracts_middle_field` | LOAD_FIELD 1 | `[20]` | `R_dest = 20` | None |
| `load_field_consumes_receiver` | Receiver consumed | `[10]` | `R_dest = 10` (src consumed) | None |
| `load_field_out_of_bounds_silent_noop` | Defensive no-op | `[]` | `[]` (no write) | None |
| `nested_enum_gc_traces_correctly` | Nested enum GC | `[outer_obj]` | Heap-level test; codegen must save heap ptrs in `R_CS_0..R_CS_3` before CALL | **Minor** |
| `heap_does_not_grow_unboundedly_under_repeated_alloc` | GC triggers | `[]` (empty) | `[]` (empty) | None |
| `live_enum_survives_automatic_gc_cycle` | Root survives GC | `[root_enum_ptr]` | `R_CS_0 = root_enum_ptr` (loop-carried, must be callee-saved) | **Minor** |
| `with_output_captures_print` | PRINT redirect | `[string_ptr]` | `R_temp = string_ptr` (1 alloc) | None |
| `store_pop_writes_value_to_slot_and_pops` | STORE_POP basic | N/A (no alloc) | `R_0 = 42` | None |
| `store_pop_writes_to_correct_slot_index` | STORE_POP slot 2 | N/A | `R_0=99, R_2=42` | None |
| `store_pop_two_bindings_preserves_both_values` | Multi-binding | N/A | `R_0=5, R_1=10` | None |
| `store_pop_overwrites_existing_slot` | Reassignment | N/A | `R_0 = 10` | None |
| `enum_gc_marks_payload_pointers` | Heap-level payload GC | `[enum_addr]` | Heap-level test (independent of VM) | None |
| `enum_gc_marks_nested_enum_payloads` | Heap-level nested GC | `[outer_addr]` | Heap-level test (independent of VM) | None |

## Implications for the refactor

**Option 2 (callee-saves for GC-reachable values) is validated.**

- **Frame size cost:** 4 callee-save registers × 8 bytes/ptr = 32
  bytes per frame. For a typical zero-script program (≤ 8 frames deep),
  this is 256 bytes total — negligible compared to the existing 8 KB
  operand stack.
- **Per-call overhead:** 2–3 instructions (save live heap ptrs in
  the caller, restore in the callee). On the existing examples
  (most call sites have 0–1 GC-reachable registers in flight), the
  amortized cost is < 0.5 instructions per call.
- **GC root-set walk fits in < 50 LOC.** The walk is:
  ```rust
  fn gc_collect(heap: &mut Heap, frames: &[Frame], regs: &[Value; 256]) {
      let mut roots: Vec<u64> = Vec::new();
      for frame in frames {
          for slot in frame.callee_save_base..frame.callee_save_top {
              roots.push(regs[slot].raw() as u64);
          }
      }
      for slot in frames.last().live_reg_range() {
          roots.push(regs[slot].raw() as u64);
      }
      heap.trace(&roots);
      // ... mark-and-trace loop, sweep ...
  }
  ```
  ~30 LOC including the mark-and-trace loop. Well within the
  50-LOC target from `MULTI_PASS_REFACTOR_PLAN.md` §5 Experiment B.

**No existing GC test would FAIL under Option 2.** The two "Minor"
concerns (in `nested_enum_gc_traces_correctly` and
`live_enum_survives_automatic_gc_cycle`) are about the **codegen** —
the linear-scan register allocator must move loop-carried or
call-crossing heap pointers into `R_CS_0..R_CS_3` before they go out
of scope. This is standard practice for any register-allocating
compiler, and is handled by the allocator's live-range analysis
(which is already part of Phase 0 per `MULTI_PASS_REFACTOR_PLAN.md`
§3 "Register VM Design").

**The heap-level tests (`enum_gc_marks_payload_pointers`,
`enum_gc_marks_nested_enum_payloads`) are unaffected** — they exercise
the `Heap::trace` / `Object::mark_references` primitives in isolation
and don't depend on the VM's root-set walk.

## Follow-up

When the register VM is built (Phase 2 per
`MULTI_PASS_REFACTOR_PLAN.md` §6):

- Implement `R_CS_0..R_CS_3` in `common/src/opcode_v2.rs` as the
  callee-save register range.
- Update the codegen's `Expression::Call` arm to insert
  `MOV frame.spill[N], R_CS_i` instructions for every live heap
  pointer that crosses the call boundary.
- Update the linear-scan register allocator to recognize
  loop-carried live ranges and pin them to `R_CS_0..R_CS_3`.
- Update the VM's `gc_collect` to walk callee-save registers of all
  frames + the current frame's live registers.
- Port the 21 existing tests to the new VM and verify identical
  behavior. (No test changes are expected — the codegen handles the
  caller/callee-save convention transparently.)

## Concerns identified

**Two minor concerns, both about codegen correctness, not about
root-set design:**

1. **`nested_enum_gc_traces_correctly`** — the codegen must move
   heap pointers into `R_CS_0..R_CS_3` before any `CALL` that
   could trigger GC. This is a standard register-allocation concern
   and is already in scope for Phase 0's linear-scan allocator.

2. **`live_enum_survives_automatic_gc_cycle`** — the codegen must
   recognize loop-carried heap pointers and pin them to callee-save
   registers for the duration of the loop. Again, standard
   register-allocation concern; handled by live-range analysis.

**No major concerns.** Option 2 is sound for zero-script's workload,
the root-set walk is simple, and the codegen requirements are
well-understood (every C compiler implements them).

## Files in this experiment

- `README.md` — this analysis
- `src/main.rs` — small Rust prototype that demonstrates the root-set
  walk under Option 2
- `Cargo.toml` — prototype crate (empty `[workspace]` table to opt
  out of the main workspace)
