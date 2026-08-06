// Expected: compile failure — append assignment is no longer supported.
fn main() {
    let xs = readonly [1, 2, 3];
    xs[] = 4;
}
