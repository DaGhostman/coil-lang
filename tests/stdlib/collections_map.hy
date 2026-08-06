use collections::map::{
    HashMap,
    hashmap_new,
    hashmap_insert,
    hashmap_get_or,
    hashmap_contains,
    hashmap_remove,
    hashmap_clear,
};

test("hashmap insert get update remove") {
    let m = hashmap_new();
    assert(hashmap_insert(m, 1, "a"))?;
    assert(hashmap_insert(m, 2, "b"))?;
    assert(hashmap_insert(m, 1, "A") == false)?;
    assert(m.size() == 2)?;
    assert(hashmap_get_or(m, 1, "?") == "A")?;
    assert(hashmap_get_or(m, 2, "?") == "b")?;
    assert(hashmap_get_or(m, 3, "?") == "?")?;
    assert(hashmap_contains(m, 2))?;
    assert(hashmap_contains(m, 9) == false)?;
    assert(hashmap_remove(m, 2))?;
    assert(hashmap_contains(m, 2) == false)?;
    assert(m.size() == 1)?;
    hashmap_clear(m);
    assert(m.is_empty())?;
}
