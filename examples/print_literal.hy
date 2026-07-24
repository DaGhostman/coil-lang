// Phase 1.6 — CFG-path `print "literal";` test.
//
// Exercises the linearizer's new `Inst::Print` arm and the
// `is_straight_line` lift for `Expression::Print`. The function
// body is straight-line (no control flow, no Call, no
// Construct, no Access), so it routes through the
// `cfg_builder` + `linearize` pipeline. The linearizer emits
// `DATA chars + STRING + PRINT` after the cfg_builder pushes
// `Inst::ConstString` for the format and `Inst::Print { args:
// [fmt] }` for the print.
//
// Expected output: "hello".

fn main() {
    print "hello";
}
