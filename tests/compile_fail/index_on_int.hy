// Expected: compile failure — cannot index non-aggregate.
fn main() {
    let x = 5;
    print "%i", x[0];
}
