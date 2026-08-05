// LICM must not hoist the LOAD that feeds ArrayLen in `while len(a) < n`.
test("while len grows without empty stack") {
    let a: [int] = [];
    while len(a) < 5 {
        a[] = len(a);
    }
    assert(len(a) == 5)?;
    assert(a[0] == 0)?;
    assert(a[4] == 4)?;
}
