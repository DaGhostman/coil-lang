// Expected: compile failure — calling a non-function value.
fn main() {
    let x = 5;
    x(1);
}
