// Expected: compile failure — duplicate same-arity same-type overload (E0121).
fn f(int x) -> int {
    return x;
}

fn f(int x) -> int {
    return x + 1;
}

fn main() {}
