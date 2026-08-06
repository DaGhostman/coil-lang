use collections::map::{HashSet};

test("hashset basics") {
    let s = HashSet::new();
    assert(s.insert("a"))?;
    assert(s.insert("b"))?;
    assert(!s.insert("a"))?;
    assert(s.size() == 2)?;
    assert(s.contains("b"))?;
    assert(!s.contains("c"))?;
    assert(s.remove("a"))?;
    assert(!s.contains("a"))?;
    s.clear();
    assert(s.is_empty())?;
}
