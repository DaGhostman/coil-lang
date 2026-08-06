use collections::tree::{
    treemap_new,
    treemap_insert,
    treemap_get_or,
    treemap_contains,
};

test("treemap insert get") {
    let t = treemap_new();
    assert(treemap_insert(t, 2, 20) == true)?;
    assert(treemap_insert(t, 1, 10) == true)?;
    assert(treemap_insert(t, 3, 30) == true)?;
    assert(treemap_insert(t, 2, 22) == false)?;
    assert(t.len == 3)?;
    assert(treemap_get_or(t, 1, -1) == 10)?;
    assert(treemap_get_or(t, 2, -1) == 22)?;
    assert(treemap_get_or(t, 3, -1) == 30)?;
    assert(treemap_contains(t, 3) == true)?;
    assert(treemap_contains(t, 9) == false)?;
}
