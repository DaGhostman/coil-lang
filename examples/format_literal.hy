// Phase 1.6 (Format lift) — CFG-path `print "%i", x;` test.
//
// Exercises the linearizer's new `Inst::Print` arm for the
// format-specifier case. The cfg_builder pushes the param
// (`42`) BEFORE the format string (`"%i"`), then
// `Inst::Print { args: [fmt, arg] }`. The linearizer emits
// `FORMAT(1)` (pops 2 values: format string + one param)
// followed by `PRINT`.
//
// The `is_straight_line` lift in `compiler/src/lib.rs`
// allows this pattern to route through the CFG path
// (previously it fell back to the single-pass codegen).
//
// Expected output: "42".

fn main() {
    print "%i", 42;
}
