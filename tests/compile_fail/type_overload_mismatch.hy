// Expected: compile failure — same-arity overloads, no candidate for bool.
fn show(int x) -> int {
    return x;
}

fn show(float x) -> float {
    return x;
}

fn main() {
    show(true);
}
