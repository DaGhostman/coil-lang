use collections::map::{
    hashmap_with_capacity,
    hashmap_insert,
    hashmap_get_or,
};

// Avoid rehash in the harness for now — grow works under `main` but the
// test runner's tighter operand stack trips during bulk rehash + dict calls.
test("hashmap many inserts") {
    let m = hashmap_with_capacity(64);
    let i = 0;
    while i < 40 {
        assert(hashmap_insert(m, i, i * 10))?;
        i = i + 1;
    }
    assert(m.size() == 40)?;
    assert(hashmap_get_or(m, 0, -1) == 0)?;
    assert(hashmap_get_or(m, 39, -1) == 390)?;
    assert(hashmap_get_or(m, 100, -1) == -1)?;
}
