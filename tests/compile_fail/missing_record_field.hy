// Expected: compile failure — missing field on dict access.
fn main() {
    let x = { foo: 42 };
    print "%i", x.bar;
}
