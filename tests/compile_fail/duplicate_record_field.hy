// Expected: compile failure — duplicate field in record literal.
fn main() {
    let x = { foo: 1, foo: 2 };
}
