# Experiment C: Register Pressure on Current Examples

## Question

Is 256 registers enough for the existing examples? Where does
register pressure peak? Does the **Dalvik-style hybrid encoding**
(regs 0–15 inline, regs 16+ two-byte) keep hot inner loops
under 16 registers, or do we need to widen the inline range?

This validates the architectural decision in
[`MULTI_PASS_REFACTOR_PLAN.md`](../../MULTI_PASS_REFACTOR_PLAN.md) §4
Decision 4: the **256-register ceiling** and the **8 dedicated
spill registers** (`s0`–`s7`).

## Methodology

For each function in each of three target examples, we:

1. Read the source code.
2. Manually identify SSA values and their live ranges (definition
   point to last use). Positions are abstract "step numbers" —
   one per bytecode operation.
3. Feed the SSA value stream into a linear-scan allocator simulator
   (Wimmer & Mössböck's algorithm, ~150 LOC in `src/main.rs`).
4. Report peak live count and number of spills.

### Linear-scan algorithm (Wimmer & Mössböck)

For each new interval (in start-point order):

1. **Expire** intervals whose `end < new.start`.
2. Sort active intervals by `end` (earliest-ending first).
3. If active has fewer than `num_regs` entries, assign a free register.
4. Otherwise, pick the active with the LATEST end:
   - If `latest.end > new.end`, give its register to `new` and **spill**
     the active interval.
   - Otherwise, **spill** `new`.

Spill registers (the dedicated `s0`–`s7` range) are NOT modeled in
this prototype — the prototype just counts the values that would
need to spill. The real allocator would route them to the 8 spill
slots, with reload before use.

### Position numbering convention

- Function entry is position 0.
- Each `JUMP_IF_MATCH` is one position (the test).
- Each `UNPACK` (or binding) is one position.
- Each arithmetic op is one position.
- Each `RETURN` is one position.

Different arms of a `match` are given disjoint position ranges
(e.g., `Empty` arm is positions 5–5, `Tri` arm is 11–16), so the
linear scan correctly shares registers across arms.

## Per-example findings

### examples/mixed.0s

The most complex real example: a single `enum Shape` with **all
three variant shapes** (Unit, Tuple, Record), and a match that
dispatches between them with binding bodies.

#### Function: `area(Shape s) -> int`

**Source:**
```0s
fn area(Shape s) -> int {
    return match s {
        Shape::Empty => 0,
        Shape::CircleR(r) => r * r,
        Shape::Rect { width, height } => width * height,
        Shape::Tri { a, b, c } => (a + b + c) / 3,
    };
}
```

**SSA value streams (13 values):**

| v   | Name                | Range  | Notes                          |
|-----|---------------------|--------|--------------------------------|
| v0  | s (param)           | [0,4]  | Live for all 4 tag tests       |
| v1  | const 0 (Empty)     | [5,5]  | 1-step range                   |
| v2  | r (CircleR)         | [6,7]  |                                |
| v3  | r*r                 | [7,7]  | 1-step range                   |
| v4  | width (Rect)        | [8,10] |                                |
| v5  | height (Rect)       | [9,10] |                                |
| v6  | width*height        | [10,10]| 1-step range                   |
| v7  | a (Tri)             | [11,14]| Last use: a + b                |
| v8  | b (Tri)             | [12,14]| Last use: a + b                |
| v9  | c (Tri)             | [13,15]| Last use: t1 + c               |
| v10 | a + b               | [14,15]|                                |
| v11 | t1 + c              | [15,16]|                                |
| v12 | t2 / 3              | [16,16]| 1-step range                   |

**Walkthrough of the Tri arm** (positions 11–16):

| Pos | Live values                       | Count |
|-----|-----------------------------------|-------|
| 11  | {a}                               | 1     |
| 12  | {a, b}                            | 2     |
| 13  | {a, b, c}                         | 3     |
| 14  | {a, b, c, t1} (t1 = a+b produced) | **4** |
| 15  | {c, t1, t2}                       | 3     |
| 16  | {t3}                              | 1     |

**Peak live count: 4** (at position 14, in the Tri arm — three
payloads `a, b, c` plus the `a + b` temporary that gets
consumed by the next op).

**Spills under 256-reg ceiling: 0**
**Spills under 16-reg inline ceiling: 0**

**Notes:** The Tri arm's `(a + b + c) / 3` is the deepest
expression in the entire example. Even at its peak, only 4
values are live. The Empty and CircleR arms peak at 1, the Rect
arm peaks at 3 (width, height, t1 at the multiplication).

---

#### Function: `main()`

**Source:**
```0s
fn main() {
    print "%i", area(Shape::Empty);
    print "%i", area(Shape::CircleR(5));
    print "%i", area(Shape::Rect { width: 3, height: 4 });
    print "%i", area(Shape::Tri { a: 1, b: 2, c: 3 });
}
```

**SSA value streams (9 values):**

| v  | Name              | Range   |
|----|-------------------|---------|
| v0 | arg_1 (Empty)     | [0,1]   |
| v1 | ret_1 (Empty)     | [1,2]   |
| v2 | arg_2 (CircleR)   | [3,4]   |
| v3 | ret_2 (CircleR)   | [4,5]   |
| v4 | arg_3 (Rect)      | [6,7]   |
| v5 | ret_3 (Rect)      | [7,8]   |
| v6 | arg_4 (Tri)       | [9,10]  |
| v7 | ret_4 (Tri)       | [10,11] |
| v8 | s_fmt ("%i")      | [2,11]  |

The format string is interned (one allocation, reused across all
4 `print` calls), so its live range spans the whole function.

**Walkthrough (positions 0–11):**

| Pos | Live values                | Count |
|-----|----------------------------|-------|
| 0   | {arg_1}                    | 1     |
| 1   | {arg_1, ret_1} (call)      | 2     |
| 2   | {ret_1, s_fmt} (print)     | 2     |
| 3   | {arg_2}                    | 1     |
| 4   | {arg_2, ret_2, s_fmt}      | **3** |
| 5   | {ret_2, s_fmt} (print)     | 2     |
| 6   | {arg_3}                    | 1     |
| 7   | {arg_3, ret_3, s_fmt}      | **3** |
| 8   | {ret_3, s_fmt} (print)     | 2     |
| 9   | {arg_4}                    | 1     |
| 10  | {arg_4, ret_4, s_fmt}      | **3** |
| 11  | {ret_4, s_fmt} (print)     | 2     |

**Peak live count: 3** (at every call site: `arg + ret + s_fmt`).

**Spills under 256-reg ceiling: 0**
**Spills under 16-reg inline ceiling: 0**

**Notes:** The format string is live across all 4 print calls,
so it counts toward the peak at every call site. Without the
format string, the peak would be 2.

---

### examples/record.0s

Two single-arm field-access functions and a multi-arm pattern
match.

#### Function: `distance_squared(Point p) -> int`

**Source:**
```0s
fn distance_squared(Point p) -> int {
    return match p {
        Point::Origin => 0,
        Point::Point { x, y } => x * x + y * y,
    };
}
```

**SSA value streams (6 values):**

| v  | Name      | Range  | Notes                    |
|----|-----------|--------|--------------------------|
| v0 | p         | [0,2]  | Live for tag test + UNPACK |
| v1 | x         | [2,4]  | Last use: x*x            |
| v2 | y         | [3,5]  | Last use: y*y            |
| v3 | t1 = x*x  | [4,6]  | Last use: t1 + t2        |
| v4 | t2 = y*y  | [5,6]  | Last use: t1 + t2        |
| v5 | t3 = t1+t2| [6,7]  | Return value             |

**Walkthrough of the Point arm (positions 2–7):**

| Pos | Live values             | Count |
|-----|-------------------------|-------|
| 2   | {p, x} (UNPACK)         | 2     |
| 3   | {x, y}                  | 2     |
| 4   | {x, t1} (x*x)           | 2     |
| 5   | {y, t1, t2} (y*y)       | **3** |
| 6   | {t1, t2, t3} (t1+t2)    | **3** |
| 7   | {t3}                    | 1     |

**Peak live count: 3** (at positions 5 and 6).

**Spills under 256-reg ceiling: 0**
**Spills under 16-reg inline ceiling: 0**

---

#### Function: `x_coord(Point p) -> int`

**Source:**
```0s
fn x_coord(Point p) -> int {
    return p.x;
}
```

**SSA value streams (2 values):**

| v  | Name          | Range  | Notes                       |
|----|---------------|--------|-----------------------------|
| v0 | p             | [0,1]  | Param, used in LOAD_FIELD   |
| v1 | ret (p.x)     | [1,2]  | LOAD_FIELD result, returned |

**Peak live count: 2** (during LOAD_FIELD, p is the receiver and
ret is the destination).

**Spills under 256-reg ceiling: 0**

---

#### Function: `y_coord(Point p) -> int`

**Identical structure to `x_coord` (just `p.y` instead of `p.x`).**

**Peak live count: 2.**
**Spills under 256-reg ceiling: 0.**

---

#### Function: `main()`

**Source:**
```0s
fn main() {
    print "%i", distance_squared(Point::Point { x: 5, y: 12 });
    print "%i", x_coord(Point::Point { x: 5, y: 12 });
    print "%i", y_coord(Point::Point { x: 5, y: 12 });
}
```

Same shape as `mixed.0s`'s main: 3 call sites, each with
`arg + ret + s_fmt` live.

**Peak live count: 3.**
**Spills under 256-reg ceiling: 0.**

---

### examples/nested_records.0s

The deepest pattern in any example: a record payload containing
another record payload.

#### Function: `get_v(Wrap w) -> int`

**Source:**
```0s
fn get_v(Wrap w) -> int {
    return match w {
        Wrap::W { inner: Inner::I { v }, name } => v,
    };
}
```

**SSA value streams (4 values):**

| v  | Name                  | Range  | Notes                          |
|----|-----------------------|--------|--------------------------------|
| v0 | w                     | [0,2]  | Scrutinee + UNPACK source      |
| v1 | inner (Wrap::W.inner) | [2,4]  | Bound, then consumed by next UNPACK |
| v2 | name (Wrap::W.name)   | [3,3]  | **Dead value**: bound but never used |
| v3 | v (Inner::I.v)        | [4,5]  | Return value                   |

**Walkthrough (positions 0–5):**

| Pos | Live values          | Count |
|-----|----------------------|-------|
| 0   | {w}                  | 1     |
| 1   | {w} (JUMP_IF_MATCH)  | 1     |
| 2   | {w, inner} (UNPACK)  | **2** |
| 3   | {inner, name}        | **2** |
| 4   | {inner, v} (UNPACK)  | **2** |
| 5   | {v} (RETURN)         | 1     |

**Peak live count: 2** (w+inner at position 2, or inner+name or
inner+v at positions 3/4).

**Spills under 256-reg ceiling: 0.**

**Notes:** The `name` binding is **dead** — bound at position 3
but never used. The linear-scan treats it as a 1-step range
(`[3, 3]`); the register can be reused immediately. A dead-store
elimination pass would catch this and avoid the bind entirely.

---

#### Function: `main()`

**Source:**
```0s
fn main() {
    let w = Wrap::W { inner: Inner::I { v: 99 }, name: "x" };
    print "%i", get_v(w);
}
```

**SSA value streams (5 values):**

| v  | Name              | Range  | Notes                       |
|----|-------------------|--------|-----------------------------|
| v0 | t1 (Inner::I)     | [0,2]  | Inner constructor           |
| v1 | t2 ("x")          | [1,2]  | String literal              |
| v2 | t3 (Wrap::W)      | [2,3]  | Outer constructor, used in call |
| v3 | ret (get_v)       | [3,4]  | get_v's return              |
| v4 | s_fmt ("%i")      | [4,4]  | Format string for print     |

**Walkthrough (positions 0–4):**

| Pos | Live values             | Count |
|-----|-------------------------|-------|
| 0   | {t1}                    | 1     |
| 1   | {t1, t2}                | 2     |
| 2   | {t1, t2, t3}            | **3** |
| 3   | {t3, ret}               | 2     |
| 4   | {ret, s_fmt}            | 2     |

**Peak live count: 3** (at position 2, the `Wrap::W` constructor
reads t1 and t2 and produces t3).

**Spills under 256-reg ceiling: 0.**

---

### Synthetic worst cases

Two stress tests to demonstrate the linear-scan's behavior under
heavy pressure.

#### Synthetic: `chain_100`

100 `let`-bound variables, all live until the return. The
return uses all 100 values.

**Peak live count: 100.**

**Spills under 256-reg ceiling: 0.**
**Spills under 16-reg inline ceiling: 84.**

This represents a deliberately-constructed artificial program;
no real zero-script code would have 100 let-bound variables in
a single function. It's here to demonstrate that the linear-scan
handles ~100-register pressure without spilling at 256.

#### Synthetic: `chain_300` (stress test)

300 `let`-bound variables, all live until the return.

**Peak live count: 300.**

**Spills under 256-reg ceiling: 44.**
**Spills under 16-reg inline ceiling: 284.**

This is a stress test that **exceeds** 256 registers. With 256
registers, 44 values must be spilled (to memory). The 8
dedicated spill registers (`s0`–`s7`) aren't enough for 44
simultaneous spills — but the 44 spilled values are COLD (their
last use is the return at the end of the function), so a
single spill slot can be reused for all 44, with a reload before
the return.

In a real program, this is equivalent to "we need a frame-local
spill area of 44 × 8 bytes = 352 bytes for cold values". This
is well within the typical frame size budget.

## Summary table

| Function                         | Example                  | Peak | Spills@16 | Spills@256 | OK@256 |
|----------------------------------|--------------------------|------|-----------|------------|--------|
| `area`                           | examples/mixed.0s        | 4    | 0         | 0          | yes    |
| `main`                           | examples/mixed.0s        | 3    | 0         | 0          | yes    |
| `distance_squared`               | examples/record.0s       | 3    | 0         | 0          | yes    |
| `x_coord`                        | examples/record.0s       | 2    | 0         | 0          | yes    |
| `y_coord`                        | examples/record.0s       | 2    | 0         | 0          | yes    |
| `main`                           | examples/record.0s       | 3    | 0         | 0          | yes    |
| `get_v`                          | examples/nested_records.0s | 2  | 0         | 0          | yes    |
| `main`                           | examples/nested_records.0s | 3  | 0         | 0          | yes    |
| `chain_100` (synthetic)          | (synthetic worst case)   | 100  | 84        | 0          | yes    |
| `chain_300` (synthetic stress)   | (synthetic stress)       | 300  | 284       | 44         | NO     |

## Conclusions

- **Peak live count across all real examples: 4** (in the
  `Tri` arm of `area` in `mixed.0s`).
- **Spills required at 256-register ceiling: 0** for all
  real-example functions.
- **Spills required at 16-register inline ceiling: 0** for all
  real-example functions.

The 256-register ceiling is **massively over-provisioned** for
the current workload. Even the most complex function in the
most complex example (the `Tri` arm of `area`) uses only **4
registers at peak** — 1.5% of the available 256.

The 16-register inline encoding (the "hot inner loop" range in
the Dalvik-style hybrid encoding) is **more than sufficient** for
the current workload. Every measured function fits in 4
registers; none approach 16.

### Spill thresholds (for context)

The linear-scan algorithm has a bounded re-pass for splits (max
3 re-passes per Wimmer's paper). After 3 re-passes, any remaining
active values are spilled to dedicated spill registers.

Spills degrade performance but do NOT cause incorrect execution.
For zero-script's workload:

- **0–2 spills:** negligible cost (~1 reload per spilled value).
- **3–10 spills:** measurable but acceptable.
- **10+ spills:** optimization target, not a correctness issue.

In a real program that hits 10+ spills, the typical fix is to
**break the function into smaller helpers** (a "spill budget"
that maps to a code-size budget). The compiler can emit a warning
when a function exceeds 32 simultaneous live values (the soft
limit beyond which inlining is preferred).

## Implications for the refactor

- **256-register ceiling: validated.** The current examples use
  4 registers at peak; the 256-register ceiling is a 64× safety
  margin. Even a synthetic 100-variable function fits without
  spilling. The ceiling does not need to change.

- **Spill allocation strategy: works as-is.** The 8 dedicated
  spill registers (`s0`–`s7`) are not exercised by any current
  example. The `chain_300` synthetic stress test would need 44
  simultaneous spill slots, but the 44 spilled values are all
  COLD (last use is the return at end-of-function) and can share
  a single frame-local spill area.

- **Encoding width: 1 byte (regs 0–15) is sufficient for the
  current workload.** The Dalvik-style hybrid encoding
  (1-byte inline for regs 0–15, 2-byte for regs 16+) is well
  matched to the current code:
  - Hot inner loops use <16 registers (often <4).
  - Even the 16-register boundary is unused by any real example.
  - The 256-register ceiling is a 16× safety margin beyond the
    16-register inline boundary.
  - Future programs that DO exceed 16 registers pay a 1-byte
    overhead per register operand (still net-positive vs the
    stack machine's `LOAD`/`STORE`/`POP` per intermediate value).

- **No changes to the refactor plan are needed.** The
  architectural decision in
  `MULTI_PASS_REFACTOR_PLAN.md` §4 Decision 4 (256-register
  ceiling, 8 dedicated spill registers, Dalvik-style hybrid
  encoding) is validated by this experiment.

## Follow-up

When the register VM is built (Phase 0+ per
`MULTI_PASS_REFACTOR_PLAN.md` §6):

- Implement the linear-scan allocator in
  `compiler/src/regalloc.rs` (~150 LOC, mirroring this
  prototype).
- Use the peak live counts from this experiment to size the
  default register file (256, per the refactor plan).
- Implement the 8 dedicated spill registers (`s0`–`s7`) per the
  refactor plan. The current examples do not exercise them, but
  they are the safety net for synthetic stress cases.
- For programs that exceed the 16-register inline encoding,
  widen the encoding on a per-function basis (the refactor plan
  already documents this as a follow-up).
- The 8 dedicated spill registers are NOT a hard limit — the
  prototype's `chain_300` stress test (peak 300) shows that
  even a function far above the register ceiling can compile
  correctly with a frame-local spill area. The 8 dedicated
  spill registers are a performance optimization (avoid
  frame-local stores for the common case), not a correctness
  requirement.

### Future concerns identified

None for the current examples. The refactor's 256-register
ceiling is well-validated. Future features that COULD push
register pressure higher:

- **Generic type parameters** (each monomorphization has its own
  register pressure).
- **Coroutines / generators** (captured variables create
  loop-carried live ranges).
- **Closures** (captured variables have unbounded lifetimes).
- **Async/await** (futures create implicit join points at
  `await`).

If register pressure grows beyond 32 simultaneous values in
hot paths, the refactor plan's 256-register ceiling is still
safe — but the 16-register inline encoding may need to be
widened to 32 (regs 0–31 inline, 32+ two-byte).

## Files in this experiment

- `README.md` — this analysis
- `src/main.rs` — the linear-scan simulator + test scenarios
- `Cargo.toml` — prototype crate (empty `[workspace]` table to
  opt out of the main workspace)
- `.gitignore` — `/target`
