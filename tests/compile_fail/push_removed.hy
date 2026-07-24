// Expected: compile failure — `push` is no longer a builtin.
fn main() {
    let a = [1];
    push(a, 2);
}
