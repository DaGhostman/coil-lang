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

test("treemap empty and skewed inserts") {
    let t = TreeMap::empty();
    assert(t.is_empty())?;
    assert(t.size() == 0)?;
    assert(t.contains(1) == false)?;
    assert(t.get_or(1, -1) == -1)?;
    // Ascending keys only walk the right spine.
    assert(t.insert(1, 10))?;
    assert(t.insert(2, 20))?;
    assert(t.insert(3, 30))?;
    assert(t.insert(4, 40))?;
    assert(t.size() == 4)?;
    assert(t.get_or(1, -1) == 10)?;
    assert(t.get_or(4, -1) == 40)?;
    assert(t.contains(2))?;
    assert(t.contains(9) == false)?;
}

test("treemap left spine inserts") {
    let t = TreeMap::new();
    assert(t.insert(4, 40))?;
    assert(t.insert(3, 30))?;
    assert(t.insert(2, 20))?;
    assert(t.insert(1, 10))?;
    assert(t.size() == 4)?;
    assert(t.get_or(1, -1) == 10)?;
    assert(t.get_or(4, -1) == 40)?;
    assert(t.contains(2))?;
}
