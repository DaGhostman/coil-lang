// Expected: compile failure — tuple index out of bounds.
fn main() {
    let t = (1, 2);
    print "%i", t[5];
}
