use collections::map::{HashMap};

test("hashmap insert get update remove") {
    let m = HashMap::new();
    assert(m.insert(1, "a"))?;
    assert(m.insert(2, "b"))?;
    assert(m.insert(1, "A") == false)?;
    assert(m.size() == 2)?;
    assert(m.get_or(1, "?") == "A")?;
    assert(m.get_or(2, "?") == "b")?;
    assert(m.get_or(3, "?") == "?")?;
    assert(m.contains(2))?;
    assert(m.contains(9) == false)?;
    assert(m.remove(2))?;
    assert(m.contains(2) == false)?;
    assert(m.size() == 1)?;
    m.clear();
    assert(m.is_empty())?;
}
