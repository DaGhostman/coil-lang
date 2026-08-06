use collections::map::{
    hashset_new,
    hashset_insert,
    hashset_contains,
    hashset_remove,
    hashset_clear,
};

test("hashset basics") {
    let s = hashset_new();
    assert(hashset_insert(s, "a"))?;
    assert(hashset_insert(s, "b"))?;
    assert(!hashset_insert(s, "a"))?;
    assert(s.size() == 2)?;
    assert(hashset_contains(s, "b"))?;
    assert(!hashset_contains(s, "c"))?;
    assert(hashset_remove(s, "a"))?;
    assert(!hashset_contains(s, "a"))?;
    hashset_clear(s);
    assert(s.is_empty())?;
}
