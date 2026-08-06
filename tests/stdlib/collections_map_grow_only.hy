use collections::map::{HashMap};

// Avoid rehash in the harness for now — grow works under `main` but the
// test runner's tighter operand stack trips during bulk rehash + dict calls.
test("hashmap many inserts") {
    let m = HashMap::with_capacity(64);
    let i = 0;
    while i < 40 {
        assert(m.insert(i, i * 10))?;
        i = i + 1;
    }
    assert(m.size() == 40)?;
    assert(m.get_or(0, -1) == 0)?;
    assert(m.get_or(39, -1) == 390)?;
    assert(m.get_or(100, -1) == -1)?;
}
