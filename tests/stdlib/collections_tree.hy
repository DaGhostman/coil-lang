use collections::tree::{TreeMap};

test("treemap insert update") {
    let t = TreeMap::new();
    assert(t.insert(2, 20) == true)?;
    assert(t.insert(1, 10) == true)?;
    assert(t.insert(3, 30) == true)?;
    assert(t.insert(2, 22) == false)?;
    assert(t.len == 3)?;
}

test("treemap get contains") {
    let t = TreeMap::new();
    assert(t.insert(2, 20))?;
    assert(t.insert(1, 10))?;
    assert(t.insert(3, 30))?;
    assert(t.insert(2, 22) == false)?;
    assert(t.get_or(1, -1) == 10)?;
    assert(t.get_or(2, -1) == 22)?;
    assert(t.get_or(3, -1) == 30)?;
    assert(t.contains(3) == true)?;
    assert(t.contains(9) == false)?;
}
