// Expected: compile failure — `?` after raise applies to the operand.
fn main() {
    raise "err"?;
}
